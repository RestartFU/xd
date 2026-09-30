#!/usr/bin/env bash
# Build the Linux host shipped with the native Windows frontend for WSL.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/dist/windows-host}"
TARGET=x86_64-unknown-linux-musl

[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || {
  echo 'build-windows-host: Linux x86_64 is required' >&2
  exit 1
}
for command in cargo musl-gcc python3; do
  command -v "$command" >/dev/null || {
    echo "build-windows-host: $command is required" >&2
    exit 1
  }
done

export CC_x86_64_unknown_linux_musl=musl-gcc
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$("$ROOT/scripts/runner-jobs.sh" --jobs)}"
export XD_COMMIT="${XD_COMMIT:-$(git -C "$ROOT" rev-parse HEAD)}"
cargo build --locked --release --target "$TARGET" \
  --manifest-path "$ROOT/host/Cargo.toml"
mkdir -p "$OUT"
install -m0755 "$ROOT/host/target/$TARGET/release/xd-host" "$OUT/xd-host-linux"

# Inspect the ELF program headers and exercise the same framed stdio protocol
# Windows uses. A dynamically linked host would break on another WSL distro.
python3 - "$OUT/xd-host-linux" <<'PY'
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

binary = Path(sys.argv[1]).resolve()
elf = binary.read_bytes()
assert elf[:6] == b'\x7fELF\x02\x01', 'host must be a little-endian ELF64 binary'
assert struct.unpack_from('<H', elf, 18)[0] == 62, 'host must target x86_64'
offset = struct.unpack_from('<Q', elf, 32)[0]
entry_size, count = struct.unpack_from('<HH', elf, 54)
assert all(struct.unpack_from('<I', elf, offset + index * entry_size)[0] != 3
           for index in range(count)), 'host must not require a dynamic loader'
print(subprocess.check_output([str(binary), '--version'], text=True).strip())
with tempfile.TemporaryDirectory(prefix='xd-windows-host-smoke-') as directory:
    environment = dict(os.environ, HOME=directory)
    requests = ''.join(json.dumps({'op': operation, '_xd_request': index}) + '\n'
                       for index, operation in enumerate(['ping', 'tree'], 1))
    result = subprocess.run([str(binary), 'stdio', '--data', directory],
                            input=requests, capture_output=True, text=True,
                            env=environment, check=True, timeout=30)
    replies = {frame['_xd_request']: frame for frame in
               map(json.loads, result.stdout.splitlines()) if '_xd_request' in frame}
    assert replies[1]['ok'] is True, replies
    assert replies[2]['ok'] is True and 'folders' in replies[2], replies
print('Static Windows WSL host: stdio ping and tree passed')
PY

(cd "$OUT"; sha256sum xd-host-linux > xd-host-linux.sha256)
