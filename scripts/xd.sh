#!/bin/sh
#
# Launcher for the relocatable xd bundle.
#
# Everything the app needs is loaded out of this directory. The bundled loader
# is invoked with --library-path rather than exporting LD_LIBRARY_PATH on
# purpose: host tools spawned by terminal sessions must keep using host
# libraries. Bundled OpenSSL uses its own small loader wrapper.

set -e

# A shell can stay attached to a directory after that directory is deleted.
# Its cached $PWD still looks right, but every child then inherits a cwd that
# getcwd(3) cannot resolve. Recover before the bundle starts helper processes.
if ! pwd -P >/dev/null 2>&1; then
  cd "${HOME:-/}" 2>/dev/null || cd /
fi

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)

case "$(basename "$HERE")" in
  xd-dev)
    export XD_APP_ID=com.restartfu.Xd.Dev
    export XD_DATA_NAME=xd-dev
    export XD_UPDATE_CHANNEL=dev
    export XD_SETTINGS_PATH="${XDG_CONFIG_HOME:-${HOME}/.config}/xd-dev/settings.json"
    ;;
  xd-nightly)
    export XD_APP_ID=com.restartfu.Xd.Nightly
    export XD_DATA_NAME=xd-nightly
    export XD_UPDATE_CHANNEL=nightly
    export XD_SETTINGS_PATH="${XDG_CONFIG_HOME:-${HOME}/.config}/xd-nightly/settings.json"
    ;;
  *)
    export XD_APP_ID=com.restartfu.Xd
    export XD_DATA_NAME=xd
    export XD_UPDATE_CHANNEL=release
    export XD_SETTINGS_PATH="${XDG_CONFIG_HOME:-${HOME}/.config}/xd/settings.json"
    ;;
esac

# Per bundle, not just per user: dev, nightly, and release installs side by
# side would otherwise rewrite each other's caches while they are running.
RUNTIME="${XDG_RUNTIME_DIR:-/tmp}/xd-$(id -u)/$(basename "$HERE")"
mkdir -p "$RUNTIME"

# This cache stores an absolute path, so it is rewritten per launch from an
# @BUNDLE@ template; that is what keeps the bundle relocatable.
sed "s|@BUNDLE@|$HERE|g" "$HERE/etc/fonts.conf.in"    > "$RUNTIME/fonts.conf"

# Anything xd launches for the user -- a terminal, an editor -- must run in the
# host's environment, not the bundle's. Remember the values before they are
# overridden so they can be handed back to terminals, agents, and host tools.
export XD_HOST_PATH="${PATH-}"
export XD_HOST_XDG_DATA_DIRS="${XDG_DATA_DIRS-}"
export XD_HOST_LANG="${LANG-}"
export XD_HOST_LC_ALL="${LC_ALL-}"
export XD_HOST_LOCPATH="${LOCPATH-}"

# Both matter: without FONTCONFIG_PATH, fontconfig also reads the host's
# /etc/fonts. It still scans the conf.avail template dir compiled into the
# library (/usr/share/fontconfig), which on a non-Debian host may hold
# newer-format files -- harmless parse warnings on stderr, fonts still resolve.
export FONTCONFIG_PATH="$HERE/etc/fonts"
export FONTCONFIG_FILE="$RUNTIME/fonts.conf"

# Keymap data. A host without these (they are not standard outside X11
# installs) leaves the window backend with no keymap.
export XKB_CONFIG_ROOT="$HERE/share/X11/xkb"
export XLOCALEDIR="$HERE/share/X11/locale"

# The bundle carries no compiled locale data, so anything but C.UTF-8 would
# fail in setlocale() and fall back to plain C -- losing UTF-8 handling along
# with it. C.UTF-8 is built into glibc and behaves correctly, at the cost of
# locale-specific number and date formatting.
export LC_ALL=C.UTF-8
export LANG=C.UTF-8
export LOCPATH="$HERE/share/locale-data"

export XDG_DATA_DIRS="$HERE/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export XCURSOR_PATH="$HERE/share/icons:${XCURSOR_PATH:-$HOME/.icons:/usr/share/icons}"
export PATH="$HERE/bin:${PATH:-/usr/local/bin:/usr/bin:/bin}"

# Project state is served by a short-lived stdio host owned by the window.
export XD_HOST_EXECUTABLE="$HERE/libexec/xd-host"
export XD_TMUX_EXECUTABLE="$HERE/libexec/tmux"
export XD_SESSION_RUNTIME="$RUNTIME/sessions"
# The in-app updater must remain self-contained too. Its installer honors
# these paths instead of selecting unrelated host network or crypto tools.
export XD_CURL="$HERE/libexec/curl"
export XD_OPENSSL="$HERE/libexec/openssl"

export SSL_CERT_FILE="$HERE/etc/ssl/certs/ca-certificates.crt"
export OPENSSL_CONF="$HERE/etc/ssl/openssl.cnf"
export OPENSSL_MODULES="$HERE/lib/ossl-modules"
export GIT_EXEC_PATH="$HERE/libexec/git-core"
export GIT_TEMPLATE_DIR="$HERE/share/git-core/templates"
export GIT_SSL_CAINFO="$HERE/etc/ssl/certs/ca-certificates.crt"

# The window is drawn through Vulkan, and the Vulkan loader finds its driver
# through JSON manifests rather than by looking for libraries. Host Mesa ICDs
# cannot safely be loaded into this relocatable runtime: their dependencies
# may require a newer host glibc while the process deliberately uses the
# bundle's libc. Use the matching bundled Mesa drivers on AMD, Intel, and
# machines without a GPU; lavapipe gives the last case a software device.
# Proprietary NVIDIA has no bundled equivalent, so NVIDIA-only systems retain
# host discovery.
# XD_HOST_VULKAN=1 leaves discovery alone either way, and
# XD_BUNDLED_VULKAN=1 forces the bundle even on an NVIDIA system.
if [ -z "${XD_HOST_VULKAN-}" ] \
  && [ -z "${VK_DRIVER_FILES-}${VK_ICD_FILENAMES-}" ]; then
  bundled_vulkan=${XD_BUNDLED_VULKAN-}
  if [ -z "$bundled_vulkan" ]; then
    nvidia_gpu=
    for vendor_file in /sys/class/drm/card*/device/vendor; do
      [ -r "$vendor_file" ] || continue
      vendor=
      IFS= read -r vendor < "$vendor_file" || true
      if [ "$vendor" = "0x10de" ]; then
        nvidia_gpu=1
        break
      fi
    done
    [ -n "$nvidia_gpu" ] || bundled_vulkan=1
  fi

  if [ -n "$bundled_vulkan" ]; then
    mkdir -p "$RUNTIME/vulkan"
    bundled_icd=
    for template in "$HERE/etc/vulkan"/*.json.in; do
      [ -e "$template" ] || continue
      manifest="$RUNTIME/vulkan/$(basename "${template%.in}")"
      sed "s|@BUNDLE@|$HERE|g" "$template" > "$manifest"
      if [ -z "$bundled_icd" ]; then
        bundled_icd="$manifest"
      else
        bundled_icd="$bundled_icd:$manifest"
      fi
    done
    if [ -n "$bundled_icd" ]; then
      # Both names: the loader renamed this variable in 1.3.207.
      export VK_DRIVER_FILES="$bundled_icd"
      export VK_ICD_FILENAMES="$bundled_icd"
    fi
  fi
fi
exec "$HERE/lib/ld-linux-x86-64.so.2" \
     --library-path "$HERE/lib" \
     "$HERE/bin/xd" "$@"
