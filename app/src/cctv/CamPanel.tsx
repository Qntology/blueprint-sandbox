import type { ReactNode } from "react";

/** CCTV 멀티뷰의 화면 한 칸 (개선안 Part 1). 머리띠에 녹화 표시 · 채널 번호 · 제목 · OSD 정보를 둡니다 (noto-sandbox 의 카메라 카드와 같은 문법). */
export function CamPanel({ cam, title, info, live = true, className, children }: { cam: string; title: ReactNode; info?: ReactNode; live?: boolean; className?: string; children: ReactNode }) {
  return (
    <section className={`cam ${className ?? ""}`}>
      <header className="cam-head">
        <span className={live ? "rec on" : "rec"} title={live ? "사건 동기화 중" : "대기"} />
        <b className="mono cam-no">{cam}</b>
        <span className="cam-title">{title}</span>
        <span className="grow" />
        {info && <span className="cam-info mono">{info}</span>}
      </header>
      <div className="cam-body">{children}</div>
    </section>
  );
}
