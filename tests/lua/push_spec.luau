--!nocheck
-- Behavioural tests for the plugin's push engine. Run with tests/lua/run.sh.
-- Expects `Mock` (mock_roblox) and `P` (exports from the plugin) in scope.

local failures = 0
local passed = 0

local function test(name, fn)
	Mock.recordings = {}
	Mock.failNameSetFor = {}
	Mock.running = false
	for _, svc in pairs(Mock.services) do
		if svc.ClassName == "Workspace" or svc.ClassName == "ServerStorage" or svc.ClassName == "ReplicatedStorage" then
			for _, child in ipairs(svc:GetChildren()) do
				child.Parent = nil
			end
			svc.__attrs = {}
		end
	end
	local ok, err = xpcall(fn, debug.traceback)
	if ok then
		passed += 1
		print("ok   - " .. name)
	else
		failures += 1
		print("FAIL - " .. name .. "\n" .. tostring(err))
	end
end

local function expect(cond, msg)
	if not cond then
		error(msg or "expectation failed", 2)
	end
end

local function node(name, className, children, extra)
	local n = { name = name, className = className, children = children or {}, properties = {}, attributes = {}, tags = {} }
	for k, v in pairs(extra or {}) do
		n[k] = v
	end
	return n
end

local function tree(services)
	return { name = "game", className = "DataModel", children = services }
end

local RS = Mock.services.ReplicatedStorage
local WS = Mock.services.Workspace
local SS = Mock.services.ServerStorage

test("fresh push creates and marks instances, leaves unmanaged content alone", function()
	local handmade = Mock.newInstance("Part", "Handmade")
	handmade.Parent = WS
	local result = P.pushTree(tree({
		node("ReplicatedStorage", "ReplicatedStorage", {
			node("Shared", "ModuleScript", {}, { source = "return 1" }),
			node("Folder", "Folder", { node("Inner", "StringValue") }),
		}),
		node("Workspace", "Workspace", {}),
	}), false, {})
	expect(result.counters.created == 3, "created=" .. result.counters.created)
	local shared = RS:FindFirstChild("Shared")
	expect(shared and shared.Source == "return 1", "source applied")
	expect(shared:GetAttribute("NyjoManagedBy") == "nyjo", "managed marker")
	expect(handmade.Parent == WS, "handmade part untouched")
	expect(#Mock.recordings == 1 and Mock.recordings[1].finished == "Enum.FinishRecordingOperation.Commit", "recorded + committed")
end)

local function seed()
	P.pushTree(tree({
		node("ReplicatedStorage", "ReplicatedStorage", {
			node("Keep", "ModuleScript", {}, { source = "return 'keep'" }),
			node("Gone", "ModuleScript", {}, { source = "return 'gone'" }),
		}),
	}), false, {})
	return RS:FindFirstChild("Gone")
end

test("removed local file is LEFT in Studio by default", function()
	local gone = seed()
	local result = P.pushTree(tree({
		node("ReplicatedStorage", "ReplicatedStorage", { node("Keep", "ModuleScript") }),
	}), false, {})
	expect(gone.Parent == RS, "still parented to ReplicatedStorage")
	expect(result.counters.deleted == 0 and result.counters.keptDeletes == 1, "kept=" .. tostring(result.counters.keptDeletes))
	expect(string.find(result.message, "LEFT in Studio", 1, true), "message explains")
	-- Still tracked, so a later allowDeletes push can clean it up.
	local again = P.pushTree(tree({
		node("ReplicatedStorage", "ReplicatedStorage", { node("Keep", "ModuleScript") }),
	}), false, { allowDeletes = true })
	expect(again.counters.deleted == 1, "deleted on explicit allowDeletes push")
end)

test("allowDeletes moves instances to ServerStorage.NyjoTrash instead of destroying", function()
	local gone = seed()
	local result = P.pushTree(tree({
		node("ReplicatedStorage", "ReplicatedStorage", { node("Keep", "ModuleScript") }),
	}), false, { allowDeletes = true })
	expect(result.counters.deleted == 1, "deleted=1")
	expect(not rawget(gone, "__destroyed"), "not destroyed")
	local trash = SS:FindFirstChild("NyjoTrash")
	expect(trash and gone.Parent and gone.Parent.Parent == trash, "inside a NyjoTrash batch folder")
	expect(gone:GetAttribute("NyjoOriginalPath") == "ReplicatedStorage.Gone", "original path recorded")
	expect(gone:GetAttribute("NyjoManagedBy") == nil, "no longer managed")
end)

test("unmanaged Studio instances are never deleted even with allowDeletes", function()
	seed()
	local manual = Mock.newInstance("ModuleScript", "Manual")
	manual.Parent = RS
	P.pushTree(tree({ node("ReplicatedStorage", "ReplicatedStorage", {}) }), false, { allowDeletes = true })
	expect(manual.Parent == RS, "manual module survives")
end)

test("mass delete is refused before anything changes", function()
	local big = {}
	for i = 1, 260 do
		table.insert(big, node("Part" .. i, "Part"))
	end
	P.pushTree(tree({ node("Workspace", "Workspace", { node("Map", "Model", big) }) }), false, {})
	local map = WS:FindFirstChild("Map")
	expect(map and #map:GetChildren() == 260, "seeded")
	local recordingsBefore = #Mock.recordings
	local ok, err = pcall(P.pushTree, tree({ node("Workspace", "Workspace", {}) }), false, { allowDeletes = true })
	expect(not ok and string.find(tostring(err), "Push refused", 1, true), "refused: " .. tostring(err))
	expect(map.Parent == WS, "map still there")
	expect(#Mock.recordings == recordingsBefore, "no recording started")
end)

test("an error mid-push cancels the recording (Studio rolls back)", function()
	Mock.failNameSetFor["Boom"] = true
	local ok, err = pcall(P.pushTree, tree({
		node("ReplicatedStorage", "ReplicatedStorage", { node("Boom", "Folder") }),
	}), false, {})
	expect(not ok and string.find(tostring(err), "rolled back", 1, true), tostring(err))
	expect(Mock.recordings[#Mock.recordings].finished == "Enum.FinishRecordingOperation.Cancel", "cancelled")
end)

test("push refuses to run during a playtest", function()
	Mock.running = true
	local ok, err = pcall(P.pushTree, tree({ node("Workspace", "Workspace", {}) }), false, {})
	expect(not ok and string.find(tostring(err), "playtest", 1, true), tostring(err))
end)

test("duplicate and invalid local nodes are reported, not applied", function()
	local result = P.pushTree(tree({
		node("ReplicatedStorage", "ReplicatedStorage", {
			node("Dup", "Folder"),
			node("Dup", "Folder"),
			{ name = "NoClass" },
		}),
	}), false, {})
	local dupCount = 0
	for _, child in ipairs(RS:GetChildren()) do
		if child.Name == "Dup" then
			dupCount += 1
		end
	end
	expect(dupCount == 1, "one Dup")
	expect(RS:FindFirstChild("NoClass") == nil, "invalid node skipped")
	local text = table.concat(result.warnings, "\n")
	expect(string.find(text, "more than one local", 1, true), "duplicate warning")
	expect(string.find(text, "no className", 1, true), "invalid warning")
end)

test("protected properties from local data are ignored", function()
	P.pushTree(tree({
		node("ReplicatedStorage", "ReplicatedStorage", {
			node("Value", "StringValue", {}, { properties = { Name = "Renamed", Value = "hi" } }),
		}),
	}), false, {})
	local v = RS:FindFirstChild("Value")
	expect(v and v.Value == "hi", "Value applied, Name kept")
end)

test("unsafe classes are skipped with a warning", function()
	local result = P.pushTree(tree({
		node("Workspace", "Workspace", { node("Ground", "Terrain"), node("Odd", "Unknownium") }),
	}), false, {})
	expect(WS:FindFirstChild("Ground") == nil and WS:FindFirstChild("Odd") == nil, "not created")
	expect(result.counters.skipped >= 2, "skipped=" .. result.counters.skipped)
end)

test("trash folder is excluded from Studio snapshots", function()
	seed()
	P.pushTree(tree({ node("ReplicatedStorage", "ReplicatedStorage", { node("Keep", "ModuleScript") }) }), false, { allowDeletes = true })
	local snapshot = P.captureStudioTree()
	for _, svc in ipairs(snapshot.children) do
		if svc.name == "ServerStorage" then
			for _, child in ipairs(svc.children) do
				expect(child.name ~= "NyjoTrash", "trash leaked into snapshot")
			end
		end
	end
end)

test("runExclusive blocks overlapping operations", function()
	local inner
	P.runExclusive("outer", function()
		inner = { pcall(P.runExclusive, "inner", function() end) }
	end)
	expect(inner[1] == false and string.find(tostring(inner[2]), "busy", 1, true), "second op refused")
	P.runExclusive("after", function() end) -- lock released
end)

