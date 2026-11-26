// The main window: the device list, the device in the middle, the inspector, and the launch
// sheets over them.

import { Icon, IconSprite } from "../components/Icon";
import { useT } from "../i18n/react";
import { showPanel, useViewer } from "../store/viewer";
import { AgentSheet } from "../features/agents/AgentSheet";
import { MoveSheet } from "../features/agents/MoveSheet";
import { Sidebar } from "../features/devices/Sidebar";
import { Inspector } from "../features/inspector/Inspector";
import { Center } from "../features/stage/Center";
import s from "./App.module.css";

export function App() {
  const t = useT();
  const sidebar = useViewer((v) => v.sidebar);
  const inspector = useViewer((v) => v.inspector);
  return (
    <>
      <IconSprite />
      <div id="app" className={s.app} data-sidebar={sidebar ? "on" : "off"} data-inspector={inspector ? "on" : "off"}>
        {/* The panel toggles sit over the headers at fixed places in the window, not inside them,
            so each stays under the pointer whether its panel is open or closed and can be clicked
            again without moving the mouse. The headers keep room for them. */}
        <button
          id="sidebar-toggle"
          className={`icon-btn ${s.pin} ${s.sidebarToggle}`}
          type="button"
          aria-label={t("sidebar.toggle")}
          title={t("sidebar.toggleHelp")}
          aria-pressed={sidebar}
          aria-controls="sidebar"
          onClick={() => showPanel("sidebar", !sidebar)}
        >
          <Icon name="i-sidebar" />
        </button>
        <button
          id="inspector-toggle"
          className={`icon-btn ${s.pin} ${s.inspectorToggle}`}
          type="button"
          aria-label={t("inspector.toggle")}
          title={t("inspector.toggleHelp")}
          aria-pressed={inspector}
          aria-controls="inspector"
          onClick={() => showPanel("inspector", !inspector)}
        >
          <Icon name="i-inspector" />
        </button>
        <Sidebar hidden={!sidebar} />
        <Center />
        <Inspector hidden={!inspector} />
      </div>
      <MoveSheet />
      <AgentSheet />
    </>
  );
}
