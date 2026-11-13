// blueprint-core: fused grouped-query attention for decoding passes (ROCm / HIP).
//
// One kernel replaces the per-layer chain of the generic path (copy the queries, a matmul per cache
// segment, concatenate, scale, softmax, copy each probability slice, a matmul per segment, add):
// about fifteen launches per layer become one.
//
// Layouts (f32, contiguous, as `ai::attention::LayerCache` keeps them):
//   q    [H, n, D]            H = Hkv * rep query heads, n new query rows
//   kt_s [Hkv, D, len_s]      keys of segment s, transposed
//   v_s  [Hkv, len_s, D]      values of segment s
//   out  [H, n, D]
// Segments 0 and 1 are the cache (base, tail), segment 2 the new tokens of this pass (len_2 == n).
// Row i sees every cached key and, of the new keys, 0..i when `causal` (else all of them).
//
// One block per (query row i, KV head g) with D threads (thread t owns output dimension t): the rep query
// heads that read KV head g share every loaded key and value. Keys are taken in chunks with an online
// softmax; f32 throughout. D must be a power of two <= MAX_D and rep <= MAX_REP (checked by the caller).

#include <hip/hip_runtime.h>

#define CHUNK 256
#define MAX_REP 8
#define MAX_D 256

extern "C" __global__ void __launch_bounds__(MAX_D)
    bp_attend(const float *__restrict__ q, const float *__restrict__ kt0, const float *__restrict__ v0, int len0,
              const float *__restrict__ kt1, const float *__restrict__ v1, int len1, const float *__restrict__ kt2,
              const float *__restrict__ v2, int len2, float *__restrict__ out, int n, int rep, int d, int causal,
              float scale) {
    __shared__ float qs[MAX_REP][MAX_D];
    __shared__ float sc[MAX_REP][CHUNK];
    __shared__ float red[MAX_REP][MAX_D];
    __shared__ float mrun[MAX_REP], lrun[MAX_REP], mnew[MAX_REP], alpha[MAX_REP];
    const int i = blockIdx.x, g = blockIdx.y, t = threadIdx.x;
    for (int r = 0; r < rep; ++r) qs[r][t] = q[((size_t)(g * rep + r) * n + i) * d + t] * scale;
    if (t < rep) {
        mrun[t] = -INFINITY;
        lrun[t] = 0.0f;
    }
    float acc[MAX_REP];
    for (int r = 0; r < MAX_REP; ++r) acc[r] = 0.0f;
    __syncthreads();

    const float *kts[3] = {kt0, kt1, kt2};
    const float *vs[3] = {v0, v1, v2};
    const int lens[3] = {len0, len1, len2};
    for (int s = 0; s < 3; ++s) {
        const int len = lens[s];
        const int visible = (s == 2 && causal) ? min(len, i + 1) : len;
        if (visible <= 0) continue;
        const float *kt = kts[s] + (size_t)g * d * len;  // [D, len] of KV head g
        const float *v = vs[s] + (size_t)g * len * d;    // [len, D]
        for (int c0 = 0; c0 < visible; c0 += CHUNK) {
            const int cl = min(CHUNK, visible - c0);
            // scores: thread t takes keys t, t + d, ...; for each dimension the threads read consecutive keys
            for (int j = t; j < cl; j += d) {
                float s_[MAX_REP];
                for (int r = 0; r < MAX_REP; ++r) s_[r] = 0.0f;
                for (int dd = 0; dd < d; ++dd) {
                    const float k = kt[(size_t)dd * len + c0 + j];
                    for (int r = 0; r < rep; ++r) s_[r] = fmaf(qs[r][dd], k, s_[r]);
                }
                for (int r = 0; r < rep; ++r) sc[r][j] = s_[r];
            }
            __syncthreads();
            // chunk maximum per head
            for (int r = 0; r < rep; ++r) {
                float m = -INFINITY;
                for (int j = t; j < cl; j += d) m = fmaxf(m, sc[r][j]);
                red[r][t] = m;
            }
            __syncthreads();
            for (int off = d / 2; off > 0; off >>= 1) {
                if (t < off)
                    for (int r = 0; r < rep; ++r) red[r][t] = fmaxf(red[r][t], red[r][t + off]);
                __syncthreads();
            }
            if (t < rep) mnew[t] = fmaxf(mrun[t], red[t][0]);
            __syncthreads();
            // exponentials and their sum per head
            for (int r = 0; r < rep; ++r) {
                float sum = 0.0f;
                for (int j = t; j < cl; j += d) {
                    const float p = expf(sc[r][j] - mnew[r]);
                    sc[r][j] = p;
                    sum += p;
                }
                red[r][t] = sum;
            }
            __syncthreads();
            for (int off = d / 2; off > 0; off >>= 1) {
                if (t < off)
                    for (int r = 0; r < rep; ++r) red[r][t] += red[r][t + off];
                __syncthreads();
            }
            if (t < rep) {
                const float a = mrun[t] == -INFINITY ? 0.0f : expf(mrun[t] - mnew[t]);
                alpha[t] = a;
                lrun[t] = lrun[t] * a + red[t][0];
                mrun[t] = mnew[t];
            }
            __syncthreads();
            // values: thread t accumulates dimension t of every head
            for (int r = 0; r < rep; ++r) acc[r] *= alpha[r];
            const float *vc = v + (size_t)c0 * d + t;
            for (int j = 0; j < cl; ++j) {
                const float x = vc[(size_t)j * d];
                for (int r = 0; r < rep; ++r) acc[r] = fmaf(sc[r][j], x, acc[r]);
            }
            __syncthreads();
        }
    }
    for (int r = 0; r < rep; ++r) out[((size_t)(g * rep + r) * n + i) * d + t] = acc[r] / lrun[r];
}
