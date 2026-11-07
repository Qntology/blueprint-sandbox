import { getCurrentWindow } from "@tauri-apps/api/window";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { loadLang } from "./i18n";
import { KioskView } from "./KioskView";
import { OfficerView } from "./OfficerView";
import "./styles.css";

// 창 이름으로 화면을 나눕니다. 현장 단말 쪽에는 주제어실 화면 코드가 그려지지 않습니다.
const isConsole = getCurrentWindow().label === "console";
document.body.classList.add(isConsole ? "theme-console" : "theme-field");
const view = isConsole ? <OfficerView /> : <KioskView />;

// 화면 언어를 먼저 읽고 그립니다 (첫 화면이 다른 언어로 깜빡이지 않게)
void loadLang().then(() => createRoot(document.getElementById("root")!).render(<StrictMode>{view}</StrictMode>));
