#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

# Check project/session/chat ownership and native shell navigation contracts.

source_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/ui/MinimalMobileApp.kt
activity_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/MainActivity.kt
chat_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/ui/ChatScreen.kt
picker_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/ui/ModelPicker.kt
client_file=mobile/shared/src/commonMain/kotlin/com/restartfu/xd/XdClient.kt

grep -Fq 'internal class MobileWorkspaceModels : ViewModel()' "$source_file" \
  || { echo "Chat and workspace stores must survive tab switches and configuration changes." >&2; exit 1; }
grep -Fq 'viewModelStoreOwner = stores.chat(activeChat.id)' "$source_file" \
  || { echo "Every chat tab must own an independent transcript and draft model." >&2; exit 1; }
grep -Fq 'workspaceOwner = stores.workspace(activeSession.id)' "$source_file" \
  || { echo "Sibling chat tabs must share their workspace pane models." >&2; exit 1; }
grep -Fq 'hiddenChatIds = hiddenChatIds + id' "$source_file" \
  || { echo "Closing a tab must hide it locally without deleting its history." >&2; exit 1; }
grep -Fq 'contentDescription = "All chats"' "$source_file" \
  || grep -Fq '"All chats"' "$source_file" \
  || { echo "Closed tabs must be discoverable in the chat menu." >&2; exit 1; }
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
grep -Fq '!chatId.startsWith("session:")' "$client_file" \
  || { echo "Shared terminal scopes must not request nonexistent chat history." >&2; exit 1; }
grep -Fq 'TERMINAL("Terminal")' "$chat_file" \
  || { echo "Native mobile chat must expose the Terminal pane." >&2; exit 1; }
grep -Fq 'factory = TerminalViewModel.ShellFactory(model.client, terminalChatId)' "$chat_file" \
  || { echo "The Terminal pane must open a shared shell for the workspace session." >&2; exit 1; }
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
