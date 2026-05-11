#!/usr/bin/env zsh

repo_root="${0:A:h:h}"

case ":$PATH:" in
	*":$repo_root/target/release:"*) ;;
	*) export PATH="$repo_root/target/release:$repo_root/target/debug:$PATH" ;;
esac

case " ${fpath[*]} " in
	*" $repo_root/shell "*) ;;
	*) fpath=("$repo_root/shell" $fpath) ;;
esac

autoload -Uz compinit
if ! whence compdef >/dev/null 2>&1; then
	compinit
fi
