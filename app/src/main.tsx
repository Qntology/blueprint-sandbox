import { getCurrentWindow } from "@tauri-apps/api/window";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { KioskView } from "./KioskView";
import { OfficerView } from "./OfficerView";
import "./styles.css";

// 창 이름으로 화면을 나눕니다. 현장 단말 쪽에는 주제어실 화면 코드가 그려지지 않습니다.
const isConsole = getCurrentWindow().label === "console";
document.body.classList.add(isConsole ? "theme-console" : "theme-field");
const view = isConsole ? <OfficerView /> : <KioskView />;

createRoot(document.getElementById("root")!).render(<StrictMode>{view}</StrictMode>);
