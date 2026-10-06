#!/bin/sh
# Runs main.mjs under the first JavaScript runtime found: $WINDHOVER_PUSH_RUNTIME, then bun or
# node (>= 20) on PATH, then their usual install locations. bora starts plugin commands with the
# server's environment, whose PATH can be minimal, so PATH alone is not enough.
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 1
for runtime in "${WINDHOVER_PUSH_RUNTIME:-}" "$(command -v bun 2>/dev/null)" "$(command -v node 2>/dev/null)" \
	"$HOME/.bun/bin/bun" /opt/homebrew/bin/bun /usr/local/bin/bun \
	/opt/homebrew/bin/node /usr/local/bin/node "$HOME/.local/bin/node" /usr/bin/node; do
	[ -n "$runtime" ] && [ -x "$runtime" ] || continue
	case "${runtime##*/}" in
	node*) "$runtime" -e 'process.exit(Number(process.versions.node.split(".")[0]) >= 20 ? 0 : 1)' 2>/dev/null || continue ;;
	esac
	exec "$runtime" "$here/main.mjs" "$@"
done
echo "windhover-push: needs bun, or node >= 20 (set WINDHOVER_PUSH_RUNTIME to its path)" >&2
exit 127
