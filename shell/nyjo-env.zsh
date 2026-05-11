#!/usr/bin/env zsh

if [[ -z "${NYJO_REPO_ROOT:-}" ]]; then
	export NYJO_REPO_ROOT="${0:A:h:h}"
fi

export NYJO_BIN_DIR="${NYJO_BIN_DIR:-$HOME/.local/bin}"
export NYJO_COMPLETION_DIR="${NYJO_COMPLETION_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/zsh/site-functions}"

case ":$PATH:" in
	*":$NYJO_BIN_DIR:"*) ;;
	*) export PATH="$NYJO_BIN_DIR:$PATH" ;;
esac

if [[ -d "$NYJO_COMPLETION_DIR" ]]; then
	case " ${fpath[*]} " in
		*" $NYJO_COMPLETION_DIR "*) ;;
		*) fpath=("$NYJO_COMPLETION_DIR" $fpath) ;;
	esac
fi

autoload -Uz compinit
if ! whence compdef >/dev/null 2>&1; then
	compinit
fi
