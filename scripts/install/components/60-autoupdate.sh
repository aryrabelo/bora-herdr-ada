# shellcheck shell=bash
# shellcheck disable=SC2034  # NAME/DESCRIPTION are read by scripts/install.sh
NAME=autoupdate
DESCRIPTION="schedules scripts/autoupdate.sh from this checkout daily at 05:00: macOS writes the user LaunchAgent ~/Library/LaunchAgents/dev.bora.autoupdate.plist (RunAtLoad=false) and loads it with launchctl; Linux writes bora-autoupdate.service and .timer to ~/.config/systemd/user/ and enables the timer"

# The job runs this checkout's scripts/autoupdate.sh by absolute path, so it
# follows `main` of THIS clone (a clone on another branch or with local edits
# is left alone and the run only logs why). PATH is the one `just install`
# ran with, captured into the job: launchd and systemd start jobs with a bare
# PATH, where cargo is missing. A checkout without cargo cannot update itself
# (autoupdate.sh refuses `bora update` over a source build, see its header),
# so the component is unavailable there instead of scheduling a job that only
# fails. 05:00 local time; a Mac asleep at 05:00 runs it on wake, and the
# systemd timer is Persistent for the same catch-up. Machines whose
# nix-darwin config already loads org.nixos.bora-autoupdate are not touched.
# Test overrides: BORA_INSTALL_LAUNCH_AGENTS_DIR, BORA_INSTALL_SYSTEMD_USER_DIR
# (where the job files go), BORA_INSTALL_LAUNCHCTL, BORA_INSTALL_SYSTEMCTL
# (the binaries driven).
AUTOUPDATE_LABEL=dev.bora.autoupdate
AUTOUPDATE_NIX_LABEL=org.nixos.bora-autoupdate
AUTOUPDATE_HOUR=5
AUTOUPDATE_TIME="$(printf '%02d:00' "$AUTOUPDATE_HOUR")"
AUTOUPDATE_UNIT=bora-autoupdate
LAUNCHCTL="${BORA_INSTALL_LAUNCHCTL:-launchctl}"
SYSTEMCTL="${BORA_INSTALL_SYSTEMCTL:-systemctl}"
LAUNCH_AGENTS_DIR="${BORA_INSTALL_LAUNCH_AGENTS_DIR:-$HOME/Library/LaunchAgents}"
SYSTEMD_USER_DIR="${BORA_INSTALL_SYSTEMD_USER_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user}"

available() {
    if [ -e "$ROOT/.git" ] && ! command -v cargo >/dev/null 2>&1; then
        echo "cargo not on PATH (autoupdate rebuilds this checkout from source; run the installer from a shell with cargo)"
        return 1
    fi
    if is_macos; then
        command -v "$LAUNCHCTL" >/dev/null 2>&1 || { echo "launchctl not found"; return 1; }
    elif [ "$(uname -s)" = Linux ]; then
        command -v "$SYSTEMCTL" >/dev/null 2>&1 || { echo "systemctl not found"; return 1; }
        "$SYSTEMCTL" --user show-environment >/dev/null 2>&1 ||
            { echo "no systemd user manager (systemctl --user)"; return 1; }
    else
        echo "macOS (launchd) or Linux (systemd --user) only"
        return 1
    fi
}

# launchd_loaded LABEL — the label is loaded in this user's launchd domain.
launchd_loaded() {
    "$LAUNCHCTL" list 2>/dev/null | awk -v label="$1" '$3 == label { found = 1 } END { exit !found }'
}

nix_managed() {
    is_macos && launchd_loaded "$AUTOUPDATE_NIX_LABEL"
}

default_on() {
    ! nix_managed
}

xml_escape() {
    local s="$1"
    s="${s//&/&amp;}"
    s="${s//</&lt;}"
    s="${s//>/&gt;}"
    printf '%s' "$s"
}

autoupdate_plist() {
    cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>$AUTOUPDATE_LABEL</string>
	<key>ProgramArguments</key>
	<array>
		<string>/bin/bash</string>
		<string>$(xml_escape "$ROOT/scripts/autoupdate.sh")</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key>
		<string>$(xml_escape "$PATH")</string>
	</dict>
	<key>StartCalendarInterval</key>
	<dict>
		<key>Hour</key>
		<integer>$AUTOUPDATE_HOUR</integer>
		<key>Minute</key>
		<integer>0</integer>
	</dict>
	<key>RunAtLoad</key>
	<false/>
</dict>
</plist>
EOF
}

# systemd_value TEXT — TEXT for a unit setting: `%` starts a specifier, so it
# is doubled everywhere.
systemd_value() {
    printf '%s' "${1//%/%%}"
}

# systemd_quoted TEXT — TEXT as a double-quoted unit word: specifiers doubled,
# `\` and `"` backslash-escaped.
systemd_quoted() {
    local s="$1"
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    printf '"%s"' "$(systemd_value "$s")"
}

# systemd_exec_word TEXT — a quoted ExecStart= word: ExecStart also expands
# `$VAR`, so `$` is doubled there. Environment= performs no variable
# expansion (systemd.exec(5)), so `$` stays single in the PATH value.
systemd_exec_word() {
    systemd_quoted "${1//\$/\$\$}"
}

autoupdate_service() {
    cat <<EOF
[Unit]
Description=bora autoupdate ($(systemd_value "$ROOT"))

[Service]
Type=oneshot
Environment=$(systemd_quoted "PATH=$PATH")
ExecStart=$(systemd_exec_word "$(command -v bash)") $(systemd_exec_word "$ROOT/scripts/autoupdate.sh")
EOF
}

autoupdate_timer() {
    printf '%s\n' "[Unit]" "Description=bora autoupdate, daily at $AUTOUPDATE_TIME" "" \
        "[Timer]" "OnCalendar=*-*-* $AUTOUPDATE_TIME:00" "Persistent=true" "" \
        "[Install]" "WantedBy=timers.target"
}

# write_if_changed PATH CONTENT — rewrites PATH only when its content differs;
# succeeds when it wrote.
write_if_changed() {
    if [ -f "$1" ] && [ "$(cat "$1")" = "$2" ]; then
        return 1
    fi
    mkdir -p "$(dirname "$1")"
    rm -f "$1"
    printf '%s\n' "$2" >"$1"
}

install_launchd() {
    local plist="$LAUNCH_AGENTS_DIR/$AUTOUPDATE_LABEL.plist" domain changed=0
    domain="gui/$(id -u)"
    if refuse_symlink autoupdate "$plist"; then
        note "write $plist yourself (label $AUTOUPDATE_LABEL, /bin/bash $ROOT/scripts/autoupdate.sh daily at $AUTOUPDATE_TIME),"
        note "then: launchctl bootstrap $domain $plist"
        return 0
    fi
    if write_if_changed "$plist" "$(autoupdate_plist)"; then changed=1; fi
    if [ "$changed" = 0 ] && launchd_loaded "$AUTOUPDATE_LABEL"; then
        say autoupdate "already present ($plist, loaded)"
        return 0
    fi
    "$LAUNCHCTL" bootout "$domain/$AUTOUPDATE_LABEL" >/dev/null 2>&1 || true
    "$LAUNCHCTL" bootstrap "$domain" "$plist"
    say autoupdate "installed ($plist, daily at $AUTOUPDATE_TIME, log: ${XDG_STATE_HOME:-$HOME/.local/state}/bora/autoupdate.log)"
}

install_systemd() {
    local service="$SYSTEMD_USER_DIR/$AUTOUPDATE_UNIT.service"
    local timer="$SYSTEMD_USER_DIR/$AUTOUPDATE_UNIT.timer" changed=0
    if refuse_symlink autoupdate "$service" || refuse_symlink autoupdate "$timer"; then
        note "write $service and $timer yourself (oneshot running $ROOT/scripts/autoupdate.sh, OnCalendar=*-*-* $AUTOUPDATE_TIME:00),"
        note "then: systemctl --user daemon-reload && systemctl --user enable --now $AUTOUPDATE_UNIT.timer"
        return 0
    fi
    if write_if_changed "$service" "$(autoupdate_service)"; then changed=1; fi
    if write_if_changed "$timer" "$(autoupdate_timer)"; then changed=1; fi
    if [ "$changed" = 0 ] &&
        "$SYSTEMCTL" --user is-enabled --quiet "$AUTOUPDATE_UNIT.timer" 2>/dev/null &&
        "$SYSTEMCTL" --user is-active --quiet "$AUTOUPDATE_UNIT.timer" 2>/dev/null; then
        say autoupdate "already present ($timer, enabled)"
        return 0
    fi
    "$SYSTEMCTL" --user daemon-reload
    "$SYSTEMCTL" --user enable --now "$AUTOUPDATE_UNIT.timer"
    say autoupdate "installed ($timer, daily at $AUTOUPDATE_TIME, log: ${XDG_STATE_HOME:-$HOME/.local/state}/bora/autoupdate.log)"
}

install_component() {
    local branch
    if nix_managed; then
        say autoupdate "already present (managed by nix-darwin: $AUTOUPDATE_NIX_LABEL)"
        return 0
    fi
    if is_macos; then install_launchd; else install_systemd; fi
    branch="$(git -C "$ROOT" symbolic-ref --short -q HEAD 2>/dev/null || true)"
    if [ "$branch" != main ]; then
        note "this checkout is on ${branch:-a detached HEAD}; autoupdate only updates a clean main and logs a skip otherwise"
    fi
}
