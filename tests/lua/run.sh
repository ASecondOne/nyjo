#!/usr/bin/env bash
# Runs the plugin's push-engine tests under the stand-alone Luau CLI with a
# mocked Roblox API. Needs `luau` on PATH (https://github.com/luau-lang/luau/releases).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
LUAU="${LUAU:-luau}"
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT

cp "$here/mock_roblox.luau" "$out/mock_roblox.luau"
{
	echo '--!nocheck'
	echo 'local Mock = require("./mock_roblox")'
	echo 'local env = {}'
	echo 'Mock.install(env)'
	echo 'local function loadPlugin()'
	echo 'local game, plugin, Enum, Instance, task, warn, DateTime = env.game, env.plugin, env.Enum, env.Instance, env.task, env.warn, env.DateTime'
	echo 'local UDim, UDim2, Color3, Vector2, Vector3, CFrame, Font, DockWidgetPluginGuiInfo = env.UDim, env.UDim2, env.Color3, env.Vector2, env.Vector3, env.CFrame, env.Font, env.DockWidgetPluginGuiInfo'
	sed 's/{{DEFAULT_PORT}}/34872/' "$repo/assets/Nyjo.lua"
	echo 'return { pushTree = pushTree, planPush = planPush, captureStudioTree = captureStudioTree, runExclusive = runExclusive }'
	echo 'end'
	echo 'local P = loadPlugin()'
	cat "$here/push_spec.luau"
} > "$out/run.luau"

cd "$out" && "$LUAU" run.luau
