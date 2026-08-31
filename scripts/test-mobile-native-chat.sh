#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

source_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/ui/MinimalMobileApp.kt
activity_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/MainActivity.kt
chat_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/ui/ChatScreen.kt
picker_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/ui/ModelPicker.kt
settings_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/MobileSettings.kt
client_file=mobile/shared/src/commonMain/kotlin/com/restartfu/xd/XdClient.kt

grep -Fq 'val experimentMode by settings.experimentMode.collectAsStateWithLifecycle()' "$source_file" \
  || { echo "Mobile session routing must observe Experiment mode." >&2; exit 1; }
grep -Fq 'if (experimentMode)' "$source_file" \
  || { echo "Mobile sessions must select their surface from Experiment mode." >&2; exit 1; }
grep -Fq 'ChatViewModel.Factory(' "$source_file" \
  || { echo "Experiment mode must create ChatViewModel." >&2; exit 1; }
grep -Fq 'ChatScreen(' "$source_file" \
  || { echo "Experiment mode must open the native chat screen." >&2; exit 1; }
grep -Fq 'LocalViewModelStoreOwner provides chatOwner' "$source_file" \
  || { echo "Native chat panes must be disposed when leaving a session." >&2; exit 1; }
grep -Fq 'TerminalViewModel.DirectFactory(' "$source_file" \
  || { echo "Regular mode must preserve the direct terminal session." >&2; exit 1; }
grep -Fq 'onDispose { terminalOwner.viewModelStore.clear() }' "$source_file" \
  || { echo "Direct terminal sessions must be disposed when inactive." >&2; exit 1; }
grep -Fq 'key(experimentMode, active.chatId)' "$source_file" \
  || { echo "Switching Experiment mode must replace the active session surface." >&2; exit 1; }

grep -Fq 'preferences.getBoolean(KEY_EXPERIMENT_MODE, false)' "$settings_file" \
  || { echo "Mobile Experiment mode must be persisted and default off." >&2; exit 1; }
grep -Fq 'putBoolean(KEY_EXPERIMENT_MODE, enabled)' "$settings_file" \
  || { echo "Mobile Experiment mode changes must be persisted." >&2; exit 1; }
grep -Fq 'Text("Experiment mode"' "$source_file" \
  || { echo "Mobile settings must label Experiment mode." >&2; exit 1; }

if grep -Fq 'Compact {' "$activity_file"; then
  echo "The native mobile UI must retain Android's normal touch-target density." >&2
  exit 1
fi

grep -Fq 'SessionTabStrip(' "$source_file" \
  || { echo "Mobile sessions must expose separate closable tabs." >&2; exit 1; }
grep -Fq 'active = active == MobileDestination.TERMINAL' "$source_file" \
  || { echo "Mobile navigation must expose a global Terminal destination." >&2; exit 1; }
grep -Fq 'MinimalGlobalTerminal(' "$source_file" \
  || { echo "The global Terminal destination must render its own shell pane." >&2; exit 1; }
grep -Fq 'TerminalViewModel.ShellFactory(' "$source_file" \
  || { echo "The global Terminal destination must own a reusable shell session." >&2; exit 1; }
grep -Fq '!chatId.startsWith("global:")' "$client_file" \
  || { echo "Global terminal scopes must not request nonexistent chat history." >&2; exit 1; }
grep -Fq 'productHeader = {' "$source_file" \
  || { echo "Native chat sessions must retain the global product navigation." >&2; exit 1; }
grep -Fq 'TERMINAL("Terminal")' "$chat_file" \
  || { echo "Native mobile chat must expose the Terminal pane." >&2; exit 1; }
grep -Fq 'factory = TerminalViewModel.Factory(model.session, state.chatId)' "$chat_file" \
  || { echo "The Terminal pane must open a normal shell for the chat." >&2; exit 1; }
if grep -Fq 'DirectAgent.fromBackend(state.backend)' "$chat_file"; then
  echo "The native Terminal pane must not open an agent terminal." >&2
  exit 1
fi
grep -Fq 'catalog.firstOrNull { it.id == currentBackend }' "$picker_file" \
  || { echo "The mobile model picker must stay within the chat's assistant." >&2; exit 1; }
if grep -Fq 'model.selectModel(backend.id, entry.id)' "$picker_file"; then
  echo "The mobile model picker must not switch assistants." >&2
  exit 1
fi
