#!/usr/bin/env zsh
set -euo pipefail

repo_root="${0:A:h:h}"
profile="release"
link_mode="symlink"
bin_dir="${NYJO_BIN_DIR:-$HOME/.local/bin}"
completion_dir="${NYJO_COMPLETION_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/zsh/site-functions}"

while [[ $# -gt 0 ]]; do
	case "$1" in
		--debug)
			profile="debug"
			shift
			;;
		--release)
			profile="release"
			shift
			;;
		--copy)
			link_mode="copy"
			shift
			;;
		--symlink)
			link_mode="symlink"
			shift
			;;
		--bin-dir)
			bin_dir="$2"
			shift 2
			;;
		--completion-dir)
			completion_dir="$2"
			shift 2
			;;
		--help|-h)
			cat <<'EOF'
Usage: shell/install-nyjo.zsh [options]

Options:
  --debug                Build/install the debug binary
  --release              Build/install the release binary (default)
  --copy                 Copy the built binary into the bin dir
  --symlink              Symlink the built binary into the bin dir (default)
  --bin-dir DIR          Install the nyjo binary into DIR
  --completion-dir DIR   Install the zsh completion file into DIR
  --help                 Show this message
EOF
			exit 0
			;;
		*)
			echo "install-nyjo.zsh: unknown option: $1" >&2
			exit 1
			;;
	esac
done

if [[ "$profile" == "release" ]]; then
	cargo build --release --manifest-path "$repo_root/Cargo.toml"
	built_binary="$repo_root/target/release/nyjo"
else
	cargo build --manifest-path "$repo_root/Cargo.toml"
	built_binary="$repo_root/target/debug/nyjo"
fi

mkdir -p "$bin_dir" "$completion_dir"

target_binary="$bin_dir/nyjo"
if [[ "$link_mode" == "copy" ]]; then
	install -m 0755 "$built_binary" "$target_binary"
else
	ln -sf "$built_binary" "$target_binary"
fi

install -m 0644 "$repo_root/shell/_nyjo" "$completion_dir/_nyjo"

cat <<EOF
Nyjo installed.

Binary:      $target_binary
Completions: $completion_dir/_nyjo

If '$bin_dir' is not already on your PATH, add this to ~/.zshrc:
  source "$repo_root/shell/nyjo-env.zsh"

Then reload zsh or run:
  source "$repo_root/shell/nyjo-env.zsh"
EOF
