#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
NIRI_CONFIG="${NIRI_CONFIG:-$CONFIG_DIR/niri/config.kdl}"
NWALL_CONFIG_DIR="$CONFIG_DIR/nwall"
CONFIG_TOML="$NWALL_CONFIG_DIR/config.toml"
SERVICE_DST="$CONFIG_DIR/systemd/user/nwalld.service"
DESKTOP_DST="$DATA_DIR/applications/dev.nwall.Picker.desktop"
DESKTOP_SRC="$ROOT/docs/dev.nwall.Picker.desktop"

MARKER_BEGIN="// === nwall (managed by scripts/install.sh) ==="
MARKER_END="// === end nwall ==="

# ── colours / chrome ─────────────────────────────────────────────────────────
USE_COLOR=1
[[ -t 1 ]] || USE_COLOR=0
[[ "${NO_COLOR:-}" != "" ]] && USE_COLOR=0

if [[ "$USE_COLOR" == "1" ]]; then
  C_DIM=$'\033[2m'
  C_BOLD=$'\033[1m'
  C_CYAN=$'\033[36m'
  C_GREEN=$'\033[32m'
  C_YELLOW=$'\033[33m'
  C_RED=$'\033[31m'
  C_RESET=$'\033[0m'
else
  C_DIM= C_BOLD= C_CYAN= C_GREEN= C_YELLOW= C_RED= C_RESET=
fi

ok()   { printf "  ${C_GREEN}✓${C_RESET} %s\n" "$*"; }
warn() { printf "  ${C_YELLOW}!${C_RESET} %s\n" "$*"; }
fail() { printf "  ${C_RED}✗${C_RESET} %s\n" "$*" >&2; }
step() { printf "\n${C_BOLD}${C_CYAN} › %s${C_RESET}\n" "$*"; }
dim()  { printf "  ${C_DIM}%s${C_RESET}\n" "$*"; }

print_banner() {
  printf '%s\n' "$C_CYAN"
  cat <<'EOF'
  ,---.   .--..--.      .--.   ____      .---.     .---.      
  |    \  |  ||  |_     |  | .'  __ `.   | ,_|     | ,_|      
  |  ,  \ |  || _( )_   |  |/   '  \  \,-./  )   ,-./  )      
  |  |\_ \|  ||(_ o _)  |  ||___|  /  |\  '_ '`) \  '_ '`)    
  |  _( )_\  || (_,_) \ |  |   _.-`   | > (_)  )  > (_)  )    
  | (_ o _)  ||  |/    \|  |.'   _    |(  .  .-' (  .  .-'    
  |  (_,_)\  ||  '  /\  `  ||  _( )_  | `-'`-'|___`-'`-'|___  
  |  |    |  ||    /  \    |\ (_ o _) /  |        \|        \ 
  '--'    '--'`---'    `---` '.(_,_).'   `--------``--------` 
EOF
  printf '%s' "$C_RESET"
  printf "  ${C_BOLD}nwall${C_RESET}  ${C_DIM}wallpaper for niri${C_RESET}\n"
  printf "  ${C_DIM}%s${C_RESET}\n" "$(printf '─%.0s' {1..48})"
}

usage() {
  cat <<EOF
Usage: scripts/install.sh [options]

  (no flags)              interactive install / uninstall menu
  --yes                   accept defaults / non-interactive
  --no-gui                CLI + daemon only
  --no-tray               build daemon without system tray
  --no-desktop            skip .desktop launcher entry
  --no-systemd            skip systemd user unit
  --no-niri               skip merging niri layer rules
  --library DIR           wallpaper library folder
  --uninstall             remove installed files
  --purge-config          with --uninstall, also remove ~/.config/nwall
  -h, --help              this help
EOF
}

# ── args ─────────────────────────────────────────────────────────────────────
YES=0
UNINSTALL=0
PURGE_CONFIG=0
WITH_GUI=1
WITH_TRAY=1
WITH_DESKTOP=1
WITH_SYSTEMD=1
MERGE_NIRI=1
LIBRARY_DIR=""
LIBRARY_SET=0
INTERACTIVE=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --yes|-y) YES=1; INTERACTIVE=0; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    --purge-config) PURGE_CONFIG=1; shift ;;
    --no-gui) WITH_GUI=0; WITH_DESKTOP=0; INTERACTIVE=0; shift ;;
    --no-tray) WITH_TRAY=0; INTERACTIVE=0; shift ;;
    --no-desktop) WITH_DESKTOP=0; INTERACTIVE=0; shift ;;
    --no-systemd) WITH_SYSTEMD=0; INTERACTIVE=0; shift ;;
    --no-niri) MERGE_NIRI=0; INTERACTIVE=0; shift ;;
    --library)
      [[ $# -ge 2 ]] || { fail "--library needs a path"; exit 1; }
      LIBRARY_DIR="$2"
      LIBRARY_SET=1
      INTERACTIVE=0
      shift 2
      ;;
    --library=*) LIBRARY_DIR="${1#*=}"; LIBRARY_SET=1; INTERACTIVE=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "unknown option: $1"; usage; exit 1 ;;
  esac
done

if [[ "$YES" == "1" ]]; then
  INTERACTIVE=0
fi

ask_yn() {
  local prompt="$1" default="${2:-y}" reply
  if [[ "$INTERACTIVE" == "0" ]]; then
    [[ "$default" == "y" ]] && return 0 || return 1
  fi
  local hint="[Y/n]"
  [[ "$default" == "n" ]] && hint="[y/N]"
  while true; do
    read -r -p "  $prompt $hint " reply || true
    reply="${reply:-$default}"
    case "${reply,,}" in
      y|yes) return 0 ;;
      n|no) return 1 ;;
      *) echo "  Please answer y or n." ;;
    esac
  done
}

ask_choice() {
  local prompt="$1"
  shift
  local options=("$@") i
  if [[ "$INTERACTIVE" == "0" ]]; then
    echo 0
    return
  fi
  echo "  $prompt" >&2
  for i in "${!options[@]}"; do
    printf "    %d) %s\n" "$((i + 1))" "${options[$i]}" >&2
  done
  local reply
  while true; do
    read -r -p "  Choice [1-${#options[@]}]: " reply || true
    if [[ "$reply" =~ ^[0-9]+$ ]] && (( reply >= 1 && reply <= ${#options[@]} )); then
      echo "$((reply - 1))"
      return
    fi
    echo "  Invalid choice." >&2
  done
}

ask_path() {
  local prompt="$1" default="$2" reply
  if [[ "$INTERACTIVE" == "0" ]]; then
    echo "$default"
    return
  fi
  read -r -p "  $prompt [$default]: " reply || true
  echo "${reply:-$default}"
}

need_cmd() {
  if command -v "$1" >/dev/null 2>&1; then
    ok "$1"
    return 0
  fi
  fail "'$1' not found on PATH (required)"
  return 1
}

soft_cmd() {
  local why="$2"
  if command -v "$1" >/dev/null 2>&1; then
    ok "$1"
  else
    warn "$1 missing — $why"
  fi
}

expand_tilde() {
  local p="$1"
  # Quote the pattern: unquoted tilde-glob would mangle $HOME paths.
  if [[ "$p" == "~" ]]; then
    echo "$HOME"
  elif [[ "$p" == "~/"* ]]; then
    echo "$HOME/${p:2}"
  else
    echo "$p"
  fi
}

set_library_in_config() {
  local dir="$1"
  local toml="$CONFIG_TOML"
  [[ -f "$toml" ]] || return 0
  if grep -qE '^[[:space:]]*library[[:space:]]*=' "$toml"; then
    local esc
    esc="$(printf '%s' "$dir" | sed 's/[&|\\]/\\&/g')"
    sed -i "s|^[[:space:]]*library[[:space:]]*=.*|library = \"$esc\"|" "$toml"
  else
    printf '\nlibrary = "%s"\n' "$dir" >>"$toml"
  fi
}

merge_niri_config() {
  local rules="$ROOT/docs/niri-layer-rules.kdl"
  [[ -f "$rules" ]] || {
    fail "missing $rules"
    exit 1
  }

  mkdir -p "$(dirname "$NIRI_CONFIG")"
  if [[ ! -f "$NIRI_CONFIG" ]]; then
    {
      echo "// Created by nwall install.sh"
      echo
      echo "$MARKER_BEGIN"
      grep -v '^//' "$rules" | sed '/^$/d'
      echo "$MARKER_END"
    } >"$NIRI_CONFIG"
    ok "created $NIRI_CONFIG with layer rules"
    dim 'add layout { background-color "transparent" } if you already have a layout block'
    return
  fi

  local tmp
  tmp="$(mktemp)"
  awk -v begin="$MARKER_BEGIN" -v end="$MARKER_END" '
    $0 == begin { skip=1; next }
    $0 == end { skip=0; next }
    !skip { print }
  ' "$NIRI_CONFIG" >"$tmp"

  {
    cat "$tmp"
    [[ -s "$tmp" ]] && echo
    echo "$MARKER_BEGIN"
    grep -v '^//' "$rules" | sed '/^$/d'
    echo "$MARKER_END"
  } >"$NIRI_CONFIG"
  rm -f "$tmp"

  if ! grep -Eq 'background-color[[:space:]]+"transparent"' "$NIRI_CONFIG"; then
    warn "tip: inside layout { } add:  background-color \"transparent\""
  fi
  ok "merged layer rules into $NIRI_CONFIG"
}

strip_niri_block() {
  [[ -f "$NIRI_CONFIG" ]] || return 0
  local tmp
  tmp="$(mktemp)"
  awk -v begin="$MARKER_BEGIN" -v end="$MARKER_END" '
    $0 == begin { skip=1; next }
    $0 == end { skip=0; next }
    !skip { print }
  ' "$NIRI_CONFIG" >"$tmp"
  mv "$tmp" "$NIRI_CONFIG"
  ok "removed managed block from $NIRI_CONFIG"
}

restart_daemon() {
  if pgrep -x nwalld >/dev/null 2>&1; then
    pkill -x nwalld || true
    sleep 0.5
  fi
  rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/nwall.sock" 2>/dev/null || true

  if command -v systemctl >/dev/null 2>&1 \
    && systemctl --user cat nwalld.service >/dev/null 2>&1; then
    systemctl --user daemon-reload
    if systemctl --user restart nwalld.service 2>/dev/null; then
      # Give it a moment to bind; report failure if it bounce-loops.
      sleep 0.6
      if systemctl --user is-active --quiet nwalld.service; then
        ok "restarted nwalld.service"
        return
      fi
      warn "nwalld.service failed to stay active — trying direct start"
    fi
  fi
  if [[ -x "$BIN_DIR/nwalld" ]]; then
    nohup "$BIN_DIR/nwalld" >/dev/null 2>&1 &
    disown || true
    sleep 0.4
    if pgrep -x nwalld >/dev/null 2>&1; then
      ok "started $BIN_DIR/nwalld"
    else
      warn "nwalld failed to start"
    fi
  else
    warn "nwalld binary missing — could not start daemon"
  fi
}

do_uninstall() {
  print_banner
  step "Uninstall"
  if [[ "$INTERACTIVE" == "1" && "$YES" == "0" ]]; then
    ask_yn "Remove nwall binaries, desktop entry, and systemd unit?" y || {
      dim "aborted"
      exit 0
    }
  fi

  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now nwalld.service 2>/dev/null || true
  fi
  pkill -x nwalld 2>/dev/null || true
  pkill -x nwall-gui 2>/dev/null || true

  for f in "$BIN_DIR/nwall" "$BIN_DIR/nwalld" "$BIN_DIR/nwall-gui" "$SERVICE_DST" "$DESKTOP_DST" \
    "$DATA_DIR/applications/nwall-gui.desktop" \
    "$DATA_DIR/applications/nwall.desktop" \
    "$CONFIG_DIR/applications/nwall-gui.desktop" \
    "$CONFIG_DIR/applications/nwall.desktop" \
    "$CONFIG_DIR/applications/dev.nwall.Picker.desktop"; do
    if [[ -e "$f" ]]; then
      rm -f "$f"
      ok "removed $f"
    fi
  done

  strip_niri_block

  if command -v update-desktop-database >/dev/null 2>&1; then
    for apps in "$DATA_DIR/applications" "$CONFIG_DIR/applications"; do
      [[ -d "$apps" ]] && update-desktop-database "$apps" >/dev/null 2>&1 || true
    done
  fi

  if [[ "$PURGE_CONFIG" == "1" ]]; then
    if [[ -d "$NWALL_CONFIG_DIR" ]]; then
      rm -rf "$NWALL_CONFIG_DIR"
      ok "removed $NWALL_CONFIG_DIR"
    fi
  else
    dim "kept $NWALL_CONFIG_DIR (pass --purge-config to remove)"
  fi

  echo
  ok "uninstall complete"
  exit 0
}

# ── main ─────────────────────────────────────────────────────────────────────
print_banner

if [[ "$UNINSTALL" == "1" ]]; then
  do_uninstall
fi

# Interactive: pick install vs uninstall up front.
if [[ "$INTERACTIVE" == "1" ]]; then
  mode="$(ask_choice "What do you want to do?" \
    "Install / update nwall" \
    "Uninstall nwall")"
  if [[ "$mode" == "1" ]]; then
    if ask_yn "Also delete config (~/.config/nwall)?" n; then
      PURGE_CONFIG=1
    fi
    do_uninstall
  fi
fi

step "Dependencies"
need_fail=0
need_cmd cargo || need_fail=1
need_cmd install || need_fail=1
need_cmd ffmpeg || need_fail=1
need_cmd ffprobe || need_fail=1
[[ "$need_fail" == "0" ]] || exit 1
soft_cmd yt-dlp "needed for Add from YouTube in the GUI"

step "Options"
if [[ "$INTERACTIVE" == "1" ]]; then
  idx="$(ask_choice "What do you want to install?" \
    "CLI + daemon + GUI" \
    "CLI + daemon only (no GTK picker)")"
  [[ "$idx" == "1" ]] && WITH_GUI=0 && WITH_DESKTOP=0

  if ask_yn "Build daemon with system tray?" y; then WITH_TRAY=1; else WITH_TRAY=0; fi

  WITH_DESKTOP=0
  if [[ "$WITH_GUI" == "1" ]]; then
    if ask_yn "Install desktop entry (app menu / launcher)?" y; then
      WITH_DESKTOP=1
    fi
  fi

  if ask_yn "Install and enable systemd user service?" y; then WITH_SYSTEMD=1; else WITH_SYSTEMD=0; fi
  if ask_yn "Merge niri layer rules into $NIRI_CONFIG?" y; then MERGE_NIRI=1; else MERGE_NIRI=0; fi

  default_lib="$(expand_tilde "${LIBRARY_DIR:-~/Pictures/Wallpapers}")"
  LIBRARY_DIR="$(ask_path "Wallpaper library folder" "$default_lib")"
  LIBRARY_SET=1
else
  dim "gui=$WITH_GUI tray=$WITH_TRAY desktop=$WITH_DESKTOP systemd=$WITH_SYSTEMD niri=$MERGE_NIRI"
  [[ -n "$LIBRARY_DIR" ]] || LIBRARY_DIR="$HOME/Pictures/Wallpapers"
fi
LIBRARY_DIR="$(expand_tilde "$LIBRARY_DIR")"

step "Build"
cd "$ROOT"
pkgs=(-p nwall -p nwalld)
[[ "$WITH_GUI" == "1" ]] && pkgs+=(-p nwall-gui)

if [[ "$WITH_TRAY" == "1" ]]; then
  cargo build --release "${pkgs[@]}"
else
  cargo build --release -p nwall
  cargo build --release -p nwalld --no-default-features
  [[ "$WITH_GUI" == "1" ]] && cargo build --release -p nwall-gui
fi
ok "release build finished"

step "Install"
mkdir -p "$BIN_DIR"
install -Dm755 "$ROOT/target/release/nwall" "$BIN_DIR/nwall"
install -Dm755 "$ROOT/target/release/nwalld" "$BIN_DIR/nwalld"
ok "nwall → $BIN_DIR/nwall"
ok "nwalld → $BIN_DIR/nwalld"
if [[ "$WITH_GUI" == "1" ]]; then
  install -Dm755 "$ROOT/target/release/nwall-gui" "$BIN_DIR/nwall-gui"
  ok "nwall-gui → $BIN_DIR/nwall-gui"
  if [[ "$WITH_DESKTOP" == "1" ]]; then
    if [[ -f "$DESKTOP_SRC" ]]; then
      install -Dm644 "$DESKTOP_SRC" "$DESKTOP_DST"
      # Remove old desktop entry aliases.
      rm -f \
        "$DATA_DIR/applications/nwall-gui.desktop" \
        "$DATA_DIR/applications/nwall.desktop" \
        "$CONFIG_DIR/applications/nwall-gui.desktop" \
        "$CONFIG_DIR/applications/nwall.desktop" \
        "$CONFIG_DIR/applications/dev.nwall.Picker.desktop"
      ok "desktop entry → $DESKTOP_DST"
      if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$DATA_DIR/applications" >/dev/null 2>&1 || true
        [[ -d "$CONFIG_DIR/applications" ]] && update-desktop-database "$CONFIG_DIR/applications" >/dev/null 2>&1 || true
        ok "refreshed desktop database"
      fi
    else
      warn "missing docs/dev.nwall.Picker.desktop — skipped"
    fi
  fi
fi

mkdir -p "$NWALL_CONFIG_DIR"
mkdir -p "$LIBRARY_DIR"
if [[ ! -f "$CONFIG_TOML" ]]; then
  install -Dm644 "$ROOT/config/example.toml" "$CONFIG_TOML"
  set_library_in_config "$LIBRARY_DIR"
  ok "wrote $CONFIG_TOML"
elif [[ "$LIBRARY_SET" == "1" ]]; then
  set_library_in_config "$LIBRARY_DIR"
  ok "updated library → $LIBRARY_DIR"
else
  ok "kept existing $CONFIG_TOML"
fi

if [[ "$MERGE_NIRI" == "1" ]]; then
  step "niri"
  merge_niri_config
fi

if [[ "$WITH_SYSTEMD" == "1" ]]; then
  step "systemd"
  install -Dm644 "$ROOT/docs/nwalld.service" "$SERVICE_DST"
  systemctl --user daemon-reload
  systemctl --user enable nwalld.service
  ok "enabled nwalld.service"
fi

step "Daemon"
restart_daemon

echo
printf "  ${C_BOLD}done${C_RESET}\n"
dim "binaries in $BIN_DIR"
[[ "$WITH_GUI" == "1" ]] && dim "open the picker:  nwall gui"
[[ "$WITH_GUI" == "0" ]] && dim "set a wallpaper:  nwall set /path/to/file"
dim "library: $LIBRARY_DIR"
echo
