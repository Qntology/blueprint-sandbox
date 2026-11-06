// 도메인 모드 (추가 개선안 Part 4.2): 1.2GW 화력발전소 ↔ 100MW AI 데이터센터.
// 화면 용어는 원장 시드의 `plant.terms` 를 그대로 씁니다 (백엔드 `domain.rs` 의 Terms 와 같은 모양).
import { createContext, useContext } from "react";

export type DomainMode = "power_plant" | "aidc";

export interface Terms {
  code: string;
  procedure: string;
  procedure_long: string;
  change: string;
  telemetry: string;
  control_room: string;
  drawing: string;
  site: string;
  field_worker: string;
  trip: string;
  supervisor: string;
  archive_prefix: string;
  permit: string;
  bypass: string;
  console_title: string;
  kiosk_title: string;
}

export const PLANT_TERMS: Terms = {
  code: "KKS",
  procedure: "AOP",
  procedure_long: "비정상 운전절차서(AOP)",
  change: "MOC",
  telemetry: "DCS",
  control_room: "주제어실",
  drawing: "P&ID",
  site: "발전소",
  field_worker: "현장 운전원",
  trip: "트립",
  supervisor: "교대감독",
  archive_prefix: "MCR",
  permit: "PTW · LOTO",
  bypass: "급수 바이패스",
  console_title: "1.2GW 발전소 관제 콘솔",
  kiosk_title: "1.2GW 발전소 현장 순시 · 정비 단말",
};

/** 영어 화면의 기본 용어 (원장을 받기 전 첫 화면). 원장을 받으면 번역 표가 바꾼 원장 용어를 씁니다. */
export const PLANT_TERMS_EN: Terms = {
  code: "KKS",
  procedure: "AOP",
  procedure_long: "Abnormal Operating Procedure (AOP)",
  change: "MOC",
  telemetry: "DCS",
  control_room: "Main Control Room",
  drawing: "P&ID",
  site: "power plant",
  field_worker: "field operator",
  trip: "trip",
  supervisor: "shift supervisor",
  archive_prefix: "MCR",
  permit: "PTW · LOTO",
  bypass: "feedwater bypass",
  console_title: "1.2GW Power Plant Control Console",
  kiosk_title: "1.2GW Power Plant Field Patrol & Maintenance Terminal",
};

export interface DomainInfo {
  mode: DomainMode;
  label: string;
  modes: { mode: DomainMode; label: string }[];
  terms: Terms;
  site: string;
}

export const DOMAIN_ICON: Record<DomainMode, string> = { power_plant: "🏭", aidc: "🖥️" };

/** 도메인마다 처음 넣어 두는 교대감독 사번 */
export const DEFAULT_SUPERVISOR: Record<DomainMode, string> = { power_plant: "OP-201", aidc: "NOC-201" };

/** 콘솔 안 어디서나 도메인 용어를 씁니다 (OfficerView 가 원장의 용어로 채웁니다). */
export const TermsContext = createContext<Terms>(PLANT_TERMS);

export function useTerms(): Terms {
  return useContext(TermsContext);
}

/** 센서 태그의 측정 코드부터 끝까지 (KKS `10LAB20CG102` → `CG102`, DCIM `DC1CDU20CG02` → `CG02`). 첫 글자가 숫자면 KKS 입니다. */
export function shortTag(tag: string): string {
  return /^[0-9]/.test(tag) ? tag.slice(7) : tag.slice(8);
}
