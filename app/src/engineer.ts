// "엔지니어 보기" (UI 단순화 U7). 끄면(기본) 결재 · 판단 화면이 코드 이름(jev_head · verdict.rs · G4_… · team_lead_cosign 등) 없이
// 평이한 말만 보이고, 켜면 지금처럼 코드 이름을 괄호로 붙입니다. 시연 영상은 켜고 찍습니다 (record.js).
// 화면 언어처럼 React 밖(상세 서랍 제목)에서도 읽으므로 모듈 변수로 두고, 이 PC 에 기억합니다.
import { useEffect, useState } from "react";

const KEY = "blueprint.engineer-view";

function read(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

let current = read();
const listeners = new Set<(on: boolean) => void>();

export function getEngineer(): boolean {
  return current;
}

export function setEngineer(on: boolean) {
  if (on === current) return;
  current = on;
  try {
    localStorage.setItem(KEY, on ? "1" : "0");
  } catch {
    // 저장소가 막혀 있으면 이번 실행에만
  }
  listeners.forEach((f) => f(on));
}

export function useEngineer(): boolean {
  const [on, setOn] = useState(current);
  useEffect(() => {
    listeners.add(setOn);
    return () => void listeners.delete(setOn);
  }, []);
  return on;
}

/** 엔지니어 보기일 때만 붙는 코드 이름 꼬리 (앞 공백 포함). 예: `${eng(" (jev_head)")}` */
export function eng(tail: string): string {
  return current ? tail : "";
}
