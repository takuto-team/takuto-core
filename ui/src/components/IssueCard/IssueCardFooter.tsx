// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/** Bottom-right action menus of an IssueCard: editor, terminal, and port mappings. */

import { EditorTerminalMenu } from "./EditorTerminalMenu";
import { PortMappingsMenu } from "./PortMappingsMenu";

type MenuKind = "port" | "editor" | "terminal";

interface Props {
  canOpenEditor: boolean;
  editorUrl: string | null;
  terminalUrl: string | null;
  ports: [number, string][];
  openMenu: MenuKind | null;
  onSetMenu: (menu: MenuKind | null) => void;
  onOpenEditor: () => void;
  onOpenTerminal: () => void;
  onCloseEditor: () => void;
  onCloseTerminal: () => void;
}

export function IssueCardFooter({
  canOpenEditor,
  editorUrl,
  terminalUrl,
  ports,
  openMenu,
  onSetMenu,
  onOpenEditor,
  onOpenTerminal,
  onCloseEditor,
  onCloseTerminal,
}: Props) {
  if (ports.length === 0 && !canOpenEditor) return null;
  return (
    <div className="absolute bottom-3 right-3 z-10 flex items-center gap-2">
      {canOpenEditor && (
        <EditorTerminalMenu
          kind="editor"
          url={editorUrl}
          isMenuOpen={openMenu === "editor"}
          onToggleMenu={(open) => onSetMenu(open ? "editor" : null)}
          onStart={onOpenEditor}
          onStop={onCloseEditor}
        />
      )}
      {canOpenEditor && (
        <EditorTerminalMenu
          kind="terminal"
          url={terminalUrl}
          isMenuOpen={openMenu === "terminal"}
          onToggleMenu={(open) => onSetMenu(open ? "terminal" : null)}
          onStart={onOpenTerminal}
          onStop={onCloseTerminal}
        />
      )}
      <PortMappingsMenu
        ports={ports}
        isMenuOpen={openMenu === "port"}
        onToggleMenu={(open) => onSetMenu(open ? "port" : null)}
      />
    </div>
  );
}
