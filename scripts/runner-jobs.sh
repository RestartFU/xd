#!/usr/bin/env bash
# Print a runner-safe build parallelism value or CPU set. By default builds
# use three quarters of the logical CPUs visible to this process. Override the
# job count with XD_BUILD_JOBS when a runner needs a stricter local limit.

set -euo pipefail

cpus=()
if [[ -r /proc/self/status ]]; then
  allowed=$(sed -nE 's/^Cpus_allowed_list:[[:space:]]*//p' /proc/self/status)
  IFS=',' read -ra ranges <<< "$allowed"
  for range in "${ranges[@]}"; do
    if [[ "$range" == *-* ]]; then
      first=${range%-*}
      last=${range#*-}
    else
      first=$range
      last=$range
    fi
    for ((cpu = first; cpu <= last; cpu++)); do
      cpus+=("$cpu")
    done
  done
fi

if ((${#cpus[@]} == 0)); then
  if command -v getconf >/dev/null 2>&1; then
    total=$(getconf _NPROCESSORS_ONLN 2>/dev/null || true)
  fi
  if [[ ! "${total:-}" =~ ^[1-9][0-9]*$ ]] \
    && command -v sysctl >/dev/null 2>&1; then
    total=$(sysctl -n hw.logicalcpu 2>/dev/null || true)
  fi
  [[ "${total:-}" =~ ^[1-9][0-9]*$ ]] || total=1
  for ((cpu = 0; cpu < total; cpu++)); do
    cpus+=("$cpu")
  done
fi

total=${#cpus[@]}
jobs=$((total * 75 / 100))
((jobs >= 1)) || jobs=1
if [[ "${XD_BUILD_JOBS:-}" =~ ^[1-9][0-9]*$ ]]; then
  jobs=$XD_BUILD_JOBS
fi
((jobs <= total)) || jobs=$total

case "${1:---jobs}" in
  --jobs)
    printf '%s\n' "$jobs"
    ;;
  --cpuset)
    selected=()
    for ((index = 0; index < jobs; index++)); do
      selected+=("${cpus[index]}")
    done
    (IFS=','; printf '%s\n' "${selected[*]}")
    ;;
  *)
    echo 'usage: runner-jobs.sh [--jobs|--cpuset]' >&2
    exit 2
    ;;
esac
