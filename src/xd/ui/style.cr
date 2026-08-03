module Xd
  module UI
    # Preserve the established visual language across the Crystal widgets.
    STYLE = <<-CSS
      :root {
        --window-bg-color: #000000;
        --window-fg-color: #f2f2f4;
        --view-bg-color: #000000;
        --view-fg-color: #f2f2f4;
        --headerbar-bg-color: #000000;
        --headerbar-fg-color: #f2f2f4;
        --headerbar-backdrop-color: #000000;
        --sidebar-bg-color: #060607;
        --sidebar-fg-color: #f2f2f4;
        --sidebar-backdrop-color: #060607;
        --secondary-sidebar-bg-color: #060607;
        --popover-bg-color: #141416;
        --dialog-bg-color: #16161b;
        --card-bg-color: #101013;
      }
      .xd-surface, .xd-surface > *, .xd-sidebar, .xd-sidebar > *,
      window, headerbar, .toolbar {
        background-color: #0a0a0c;
      }
      popover.background {
        background: none;
        background-color: transparent;
      }
      button, dropdown > button, entry, .osd {
        background-color: alpha(#ffffff, 0.05);
        border-color: alpha(#ffffff, 0.07);
      }
      button:hover, dropdown > button:hover {
        background-color: alpha(#ffffff, 0.09);
      }
      .xd-context {
        background-color: alpha(#ffffff, 0.025);
        border-radius: 0 0 14px 14px;
        padding: 4px 12px;
      }
      .xd-context label { font-size: 0.85em; }
      .xd-auth-controls {
        background-color: #101013;
        padding-left: 8px;
      }
      window {
        font-family: "DM Sans", "Inter", "Cantarell", sans-serif;
        font-size: 0.95em;
      }
      text > placeholder { margin-left: 3px; }
      headerbar {
        min-height: 42px;
        padding-top: 5px;
        padding-bottom: 5px;
        background: transparent;
        box-shadow: none;
        border: none;
      }
      .xd-header-divider {
        min-height: 2px;
        background: #2a2a2d;
      }
      headerbar button, headerbar menubutton > button {
        min-height: 26px;
        min-width: 26px;
        padding: 2px 6px;
        margin-top: 0;
        margin-bottom: 0;
      }
      paned > separator {
        min-width: 8px;
        min-height: 8px;
        border: none;
        opacity: 0;
      }
      .xd-divider-left { border-left: 1px solid #2a2a2d; }
      .xd-divider-top { border-top: 1px solid #2a2a2d; }
      .xd-divider-bottom { border-bottom: 1px solid #2a2a2d; }
      listview > row {
        min-height: 0;
        padding: 4px 8px;
        margin: 0 6px;
        border-radius: 8px;
      }
      listview > row label { padding: 0; }
      listview > row:selected {
        background: alpha(currentColor, 0.10);
      }
      listview > row:hover:not(:selected) {
        background: alpha(currentColor, 0.05);
      }
      .xd-composer button, .xd-composer togglebutton,
      .xd-composer dropdown > button {
        background: none;
        border: none;
        box-shadow: none;
        padding: 4px 10px;
      }
      .xd-composer button buttoncontent > box { border-spacing: 6px; }
      .xd-composer button label, .xd-composer dropdown label {
        color: alpha(#ffffff, 0.6);
      }
      .xd-composer button:hover label {
        color: alpha(#ffffff, 0.85);
      }
      .xd-composer button:checked {
        background: alpha(#3584e4, 0.22);
        border-radius: 8px;
      }
      .xd-composer button:checked label,
      .xd-composer button:checked image {
        color: #6bb2f8;
      }
      .xd-composer menubutton > button:checked,
      .xd-composer dropdown > button:checked {
        background: alpha(#ffffff, 0.10);
      }
      .xd-composer menubutton > button:checked label,
      .xd-composer menubutton > button:checked image,
      .xd-composer dropdown > button:checked label,
      .xd-composer dropdown > button:checked image {
        color: alpha(#ffffff, 0.95);
      }
      button:checked { background-color: alpha(#ffffff, 0.14); }
      .xd-composer button:hover, .xd-composer togglebutton:hover,
      .xd-composer dropdown > button:hover {
        background: alpha(currentColor, 0.08);
      }
      .xd-composer button.suggested-action,
      .xd-composer button.destructive-action {
        color: #ffffff;
        border-radius: 9999px;
        min-width: 28px;
        min-height: 28px;
        padding: 4px;
      }
      .xd-composer button.suggested-action { background: #3584e4; }
      .xd-composer button.destructive-action { background: #e01b24; }
      .xd-composer button.destructive-action:hover { background: #c01c28; }
      button, dropdown > button { border-radius: 8px; }
      button.flat, dropdown > button { min-height: 24px; }
      button.circular { border-radius: 9999px; }
      .card {
        border-radius: 12px;
        background-color: alpha(#ffffff, 0.06);
        border: 1px solid alpha(#ffffff, 0.05);
      }
      frame, frame > border {
        border-radius: 16px;
        background-color: alpha(#ffffff, 0.04);
        border-color: alpha(#ffffff, 0.07);
      }
      frame > box { padding: 4px; }
      textview, textview text { background: transparent; }
      popover > contents {
        background: none;
        border: none;
        box-shadow: none;
        padding: 1px;
      }
      popover listview {
        background-color: #141416;
        border: 1px solid alpha(#ffffff, 0.10);
        border-radius: 12px;
        padding: 5px;
      }
      .xd-menu {
        background-color: #141416;
        border: 1px solid alpha(#ffffff, 0.10);
        border-radius: 12px;
        padding: 6px;
      }
      .xd-menu-popover > contents, popover.menu > contents {
        background-color: #141416;
        border: 1px solid alpha(#ffffff, 0.10);
        border-radius: 12px;
        padding: 5px;
      }
      popover menuitem { border-radius: 8px; padding: 6px 10px; }
      dialog sheet, dialog sheet.background, dialog .sheet,
      .dialog-content, alertdialog > * {
        background-color: #16161b;
      }
      .dialog-content, alertdialog {
        border-radius: 14px;
        border: 1px solid alpha(#ffffff, 0.10);
      }
      alertdialog .title { font-size: 1.05em; font-weight: 700; }
      alertdialog .response-area button {
        min-height: 30px;
        border-radius: 9px;
        background-color: alpha(#ffffff, 0.06);
      }
      alertdialog .response-area button.suggested-action {
        background-color: #3584e4;
        color: #ffffff;
      }
      alertdialog .response-area button.destructive-action {
        background-color: alpha(#e01b24, 0.85);
        color: #ffffff;
      }
      row.entry, row.combo, row.action,
      preferencesgroup listview > row {
        background-color: alpha(#ffffff, 0.05);
        border-radius: 10px;
      }
      row.entry:focus-within {
        background-color: alpha(#ffffff, 0.08);
      }
      .xd-inline-image picture { border-radius: 10px; }
      .xd-image-button {
        padding: 0;
        min-width: 0;
        min-height: 0;
        background: none;
        border: none;
        box-shadow: none;
      }
      .xd-image-button:hover, .xd-image-button:active {
        background: none;
        box-shadow: none;
      }
      .xd-image-viewer { background: transparent; }
      dialog.xd-image-dialog, dialog.xd-image-dialog > *,
      dialog.xd-image-dialog sheet {
        background: none;
        background-color: transparent;
        box-shadow: none;
        outline: none;
        border: none;
      }
      popover list, popover scrolledwindow, popover viewport,
      popover box { background: none; }
      popover listview > row, popover list > row { outline: none; }
      popover list > row { border-radius: 10px; }
      popover list > row:selected {
        background: alpha(#ffffff, 0.07);
      }
      popover list > row:hover:not(:selected) {
        background: alpha(#ffffff, 0.05);
      }
      popover listview > row {
        border-radius: 10px;
        padding: 8px 12px;
      }
      popover listview > row:selected {
        background: alpha(#ffffff, 0.07);
      }
      popover listview > row:hover:not(:selected) {
        background: alpha(#ffffff, 0.05);
      }
      scrolledwindow > undershoot.top,
      scrolledwindow > undershoot.bottom,
      scrolledwindow > undershoot.left,
      scrolledwindow > undershoot.right,
      scrolledwindow > overshoot.top,
      scrolledwindow > overshoot.bottom {
        background: none;
        box-shadow: none;
      }
      scrollbar, scrollbar > range, scrollbar > range > trough,
      scrollbar trough {
        background: none;
        background-image: none;
        border: none;
        box-shadow: none;
        min-width: 0;
        margin: 0;
        padding: 0;
      }
      scrollbar slider {
        min-width: 4px;
        min-height: 4px;
        border: none;
        margin: 2px;
        border-radius: 4px;
        background: alpha(#ffffff, 0.14);
      }
      scrollbar slider:hover { background: alpha(#ffffff, 0.28); }
      .xd-choice {
        background: none;
        border: 1px solid alpha(#ffffff, 0.10);
        border-radius: 10px;
        padding: 7px 14px;
      }
      .xd-choice label { color: alpha(#ffffff, 0.65); }
      .xd-choice:hover {
        background: alpha(#ffffff, 0.05);
        border-color: alpha(#ffffff, 0.18);
      }
      .xd-choice:hover label { color: alpha(#ffffff, 0.95); }
      progressbar.xd-context-meter { font-size: 0.82em; }
      progressbar.xd-context-meter > trough,
      progressbar.xd-context-meter > trough > progress {
        min-height: 5px;
        border-radius: 5px;
      }
      tabbar { background: none; box-shadow: none; }
      tabbar tabbox {
        background: none;
        margin: 0 -12px;
        padding: 0;
        min-width: 24px;
      }
      tabbar tabbox > tabboxchild { margin: 0 -4px; min-width: 8px; }
      tabbar tabbox > separator {
        min-width: 0;
        min-height: 0;
        margin: 0;
        background: none;
        opacity: 0;
      }
      tabbar tab {
        border-radius: 0;
        margin: 0;
        padding: 5px 8px;
        min-width: 110px;
      }
      tabbar tab:selected, tabbar tab:checked {
        background: alpha(#ffffff, 0.10);
      }
      tabbar tab button {
        opacity: 0;
        min-width: 0;
        min-height: 0;
        padding: 0;
        margin: 0;
        border: none;
      }
      .xd-code {
        background-color: alpha(#ffffff, 0.04);
        border: 1px solid alpha(#ffffff, 0.06);
        border-radius: 10px;
        padding: 10px 12px;
      }
      .xd-status {
        background-color: alpha(#3584e4, 0.08);
        border: 1px solid alpha(#3584e4, 0.22);
        border-radius: 10px;
      }
      .xd-workflow-log {
        padding: 8px 10px;
        background: alpha(#000000, 0.18);
        border-radius: 7px;
        font-family: "JetBrains Mono", monospace;
        font-size: 0.90em;
      }
      .xd-subagent {
        background-color: alpha(#a56de2, 0.07);
        border: 1px solid alpha(#a56de2, 0.22);
        border-left: 3px solid alpha(#a56de2, 0.72);
        border-radius: 10px;
      }
      button.xd-subagent-toggle,
      button.xd-subagent-toggle:hover,
      button.xd-subagent-toggle:checked {
        background: none;
        border: none;
        border-radius: 10px;
        padding: 0;
      }
      .xd-code label {
        font-family: "JetBrains Mono", monospace;
        font-size: 1em;
      }
      .xd-code textview.xd-diff,
      .xd-code textview.xd-diff text {
        background: transparent;
        font-size: 1em;
      }
      .xd-code.xd-inline-diff { padding: 0; }
      .xd-diff-text {
        min-width: 480px;
        padding: 7px 10px;
        font-family: "JetBrains Mono", monospace;
        font-size: 1em;
        line-height: 1;
      }
      listview.xd-diff-list {
        padding-top: 7px;
        padding-bottom: 7px;
        background: transparent;
      }
      .xd-diff-text.xd-diff-chunk {
        padding-top: 0;
        padding-bottom: 0;
      }
      listview.xd-diff-list > row {
        min-height: 0;
        margin: 0;
        padding: 0;
        border-radius: 0;
      }
      listview.xd-diff-list > row:hover { background: none; }
      .xd-diff-expander > box > title {
        padding: 9px 12px;
        background-color: #2a2a2d;
      }
      .xd-file-list { padding: 5px; background: transparent; }
      .xd-file-list > row { border-radius: 8px; margin: 2px 0; }
      .xd-file-list > row:selected {
        background: alpha(#ffffff, 0.09);
      }
      .xd-file-list > row:hover:not(:selected) {
        background: alpha(#ffffff, 0.05);
      }
      textview.xd-file-preview, textview.xd-file-preview text {
        background: transparent;
        font-family: "JetBrains Mono", monospace;
        font-size: 0.94em;
      }
      .xd-body { caret-color: transparent; }
      @keyframes xd-pulse {
        from { opacity: 1; }
        to { opacity: 0.25; }
      }
      .xd-status-dot {
        min-width: 7px;
        min-height: 7px;
        border-radius: 999px;
        border: 1px solid #0a0a0c;
      }
      .xd-status-waiting {
        background-color: alpha(#ffffff, 0.55);
      }
      .xd-status-done { background-color: @success_color; }
      .xd-offline { color: @error_color; }
      .xd-sidebar listview { padding-top: 0.5em; }
      /* Symbolic icons inherit the selected row's accent colour by default.
         Keep the monochrome OpenAI mark neutral in every GTK theme/state. */
      .xd-sidebar .xd-backend-codex,
      .xd-sidebar row:selected .xd-backend-codex {
        color: #bebebe;
      }
      .xd-inline-entry { min-height: 0; padding: 0 4px; }
      .xd-sidebar-row-action {
        min-width: 20px;
        min-height: 20px;
        padding: 2px;
      }

      /* Temporary adapters for Crystal rows while their C widgets are ported. */
      .xd-tool-panel, .xd-terminal { background: #0a0a0c; }
      .xd-terminal { color: #f2f2f4; padding: 8px; }
      .xd-sidebar button.flat { background: transparent; border: 0; }
      /* Stripping a flat button bare is right inside a row, where a surface
         on every rename and delete would be noise, and wrong in the bar under
         the tree: those are the window's own controls, and next to the update
         button they have to read as the same kind of thing. */
      .xd-sidebar-tools button {
        min-height: 26px;
        min-width: 26px;
        padding: 2px 6px;
      }
      .xd-sidebar-tools button.flat {
        background-color: alpha(#ffffff, 0.05);
        border: 1px solid alpha(#ffffff, 0.07);
      }
      .xd-sidebar-tools button.flat:hover {
        background-color: alpha(#ffffff, 0.09);
      }
      .xd-search-result { border-radius: 10px; padding: 10px 12px; }
      .xd-search-result:hover { background: alpha(#ffffff, 0.07); }
      .xd-message-diff {
        background: alpha(#33d17a, 0.07);
        color: #d7f8e5;
        font-family: "JetBrains Mono", monospace;
        font-size: 0.9em;
      }
      .xd-workflow-jobs {
        margin-top: 3px;
      }
      .xd-workflow-job {
        background: alpha(#000000, 0.16);
        border-radius: 8px;
        padding: 6px 8px;
      }
      .xd-workflow-job-name { font-weight: 600; }
      .xd-workflow {
        background: alpha(#ffffff, 0.045);
        border: 1px solid alpha(#ffffff, 0.09);
        border-radius: 12px;
        margin: 0 14px;
        padding: 12px 14px;
      }
      button.xd-workflow-run-link,
      button.xd-workflow-run-link:hover {
        min-height: 0;
        padding: 0 2px;
        background: none;
        border: none;
        font-weight: 700;
      }
      .xd-update-progress {
        font-size: 0.85em;
        font-weight: 700;
        font-variant-numeric: tabular-nums;
      }
      .xd-workflow-status { font-weight: 600; }
      .xd-workflow-elapsed {
        color: #a8a8a5;
        font-size: 0.88em;
        font-variant-numeric: tabular-nums;
      }
      .xd-workflow-running { color: #99c1f1; }
      .xd-workflow-success { color: #8ff0a4; }
      .xd-workflow-failure { color: #ff7b63; }
      .xd-workflow-finished { color: #c0bfbc; }
      .xd-message-error {
        background: alpha(#e01b24, 0.12);
        color: #ffb4ab;
      }
      .xd-panel {
        background: #0b0b0b;
        border-radius: 14px;
        border: 1px solid alpha(#ffffff, 0.07);
        box-shadow: 0 24px 64px alpha(#000000, 0.65);
      }
      .xd-panel-bar { padding: 13px 16px; }
      .xd-panel-head {
        border-bottom: 1px solid alpha(#ffffff, 0.06);
      }
      .xd-panel-foot {
        border-top: 1px solid alpha(#ffffff, 0.06);
      }
      .xd-panel-action {
        background: alpha(#ffffff, 0.10);
        border: 1px solid alpha(#ffffff, 0.08);
        border-radius: 9px;
        padding: 5px 14px;
        box-shadow: none;
      }
      .xd-panel-action:hover {
        background: alpha(#ffffff, 0.16);
      }
      .xd-key {
        font-size: 85%;
        padding: 1px 6px;
        border-radius: 6px;
        background: alpha(#ffffff, 0.09);
      }
      .xd-secret-name {
        font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
        font-size: 0.9em;
      }
      .xd-auth-code {
        font-family: "JetBrains Mono", monospace;
        font-size: 1.2em;
        font-weight: 700;
      }
      .xd-browser scrolledwindow,
      .xd-browser listview {
        background: transparent;
      }
      .xd-browser listview > row {
        border-radius: 9px;
        margin: 1px 8px;
        padding: 7px 10px;
      }
      .xd-browser listview > row:hover {
        background: alpha(#ffffff, 0.05);
      }
      .xd-browser listview > row:selected {
        background: alpha(#ffffff, 0.11);
        color: inherit;
      }
      .xd-browser-path {
        font-family: monospace;
        font-size: 96%;
        color: alpha(#ffffff, 0.85);
      }
      CSS
  end
end
