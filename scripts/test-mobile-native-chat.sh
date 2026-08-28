#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

source_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/ui/MinimalMobileApp.kt
activity_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/MainActivity.kt
chat_file=mobile/androidApp/src/main/kotlin/com/restartfu/xd/mobile/ui/ChatScreen.kt

grep -Fq 'ChatViewModel.Factory(' "$source_file" \
  || { echo "Minimal mobile sessions must create ChatViewModel." >&2; exit 1; }
grep -Fq 'ChatScreen(' "$source_file" \
  || { echo "Minimal mobile sessions must open the native chat screen." >&2; exit 1; }
grep -Fq 'LocalViewModelStoreOwner provides chatOwner' "$source_file" \
  || { echo "Native chat panes must be disposed when leaving a session." >&2; exit 1; }

if grep -Fq 'TerminalViewModel.DirectFactory(' "$source_file"; then
  echo "Minimal mobile sessions must not open directly into a terminal." >&2
  exit 1
fi

if grep -Fq 'Compact {' "$activity_file"; then
  echo "The native mobile UI must retain Android's normal touch-target density." >&2
  exit 1
fi

grep -Fq 'mutableStateOf(Pane.CHAT)' "$chat_file" \
  || { echo "Mobile sessions must default to the native Chat pane." >&2; exit 1; }
grep -Fq 'DirectAgent.fromBackend(state.backend)' "$chat_file" \
  || { echo "The optional terminal tab must retain access to agent sessions." >&2; exit 1; }
