#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

source_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/ui/MinimalMobileApp.kt
activity_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/MainActivity.kt
chat_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/ui/ChatScreen.kt
settings_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/MobileSettings.kt

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

if grep -Fq 'TERMINAL("Terminal")' "$chat_file" || grep -Fq 'TerminalViewModel.Factory(' "$chat_file"; then
  echo "Experiment chat must not expose a raw agent terminal pane." >&2
  exit 1
fi
