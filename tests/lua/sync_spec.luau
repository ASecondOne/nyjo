--!nocheck
-- Tests for the 1s heartbeat ("conversation") with the server, drift
-- detection and connection freshness. Concatenated after push_spec.luau by
-- run.sh, so `test`, `expect`, `node`, `tree`, `Mock` and `P` are in scope.

local requests = {}

local function reply(data, status)
	status = status or 200
	return {
		Success = status >= 200 and status < 300,
		StatusCode = status,
		Body = Mock.encode({ ok = status < 300, data = data }),
	}
end

-- routes: path -> function(body) -> response
local function serve(routes)
	requests = {}
	Mock.httpHandler = function(req)
		local path = string.match(req.Url, "^http://127%.0%.0%.1:%d+(.*)$")
		local body = req.Body and Mock.decode(req.Body) or nil
		table.insert(requests, { path = path, body = body })
		local handler = routes[path]
		if handler == nil then
			return { Success = false, StatusCode = 404, Body = "not found" }
		end
		return handler(body)
	end
end

local function lastRequest(path)
	for index = #requests, 1, -1 do
		if requests[index].path == path then
			return requests[index]
		end
	end
	return nil
end

local function resetSync()
	local s = P.syncState
	s.lastSyncedRevision = nil
	s.serverRevision = nil
	s.studioChanges = 0
	s.lastReplyMs = nil
	s.scanError = nil
	s.recentChanges = nil
	s.connectionError = nil
	s.commandRunning = false
	s.quietUntilMs = 0
	Mock.runSpawned = false
	Mock.nowMs += 10000
end

local function syncReply(revision, extra)
	return function()
		local data = { project = { revision = revision, recentChanges = {} } }
		for key, value in pairs(extra or {}) do
			data[key] = value
		end
		return reply(data)
	end
end

local function health()
	return reply({ bridgeProtocol = "nyjo-studio-bridge/2" })
end

test("heartbeat back-off is capped at 2s and resets on first reply", function()
	expect(P.nextHeartbeatDelay(true, 1) == 1)
	expect(P.nextHeartbeatDelay(false, 1) == 2, "first miss -> 2s")
	expect(P.nextHeartbeatDelay(false, 2) == 2, "capped at 2s")
	expect(P.nextHeartbeatDelay(false, 15) == 2, "old 15s back-off can never return")
	expect(P.nextHeartbeatDelay(true, 2) == 1, "snaps back to 1s")
end)

test("one sync round trip carries state and reads the server revision", function()
	resetSync()
	serve({ ["/api/plugin/sync"] = syncReply(5) })
	expect(P.bridgeSyncOnce() == true)
	expect(#requests == 1, "a single request per heartbeat (no separate poll)")
	local body = requests[1].body
	expect(type(body.sessionId) == "string" and body.sessionId ~= "")
	expect(body.bridgeVersion == "nyjo-studio-bridge/2")
	expect(body.busy == false)
	expect(body.studioChangesSinceSync == 0)
	expect(body.lastSyncedRevision == nil)
	expect(P.syncState.serverRevision == 5)
	local state = P.describeSyncState()
	expect(state == "unsynced", state)
end)

test("a command in the heartbeat reply runs immediately and reports back", function()
	resetSync()
	Mock.runSpawned = true
	serve({
		["/api/plugin/sync"] = syncReply(5, { command = { id = 3, kind = "previewPull" } }),
		["/api/health"] = health,
		["/api/sync-from-studio"] = function()
			return reply({ mode = "preview", applied = false, plan = { stats = {}, operations = {} } })
		end,
		["/api/plugin/command-result"] = function()
			return reply({})
		end,
	})
	expect(P.bridgeSyncOnce() == true)
	local result = lastRequest("/api/plugin/command-result")
	expect(result ~= nil, "command result posted")
	expect(result.body.commandId == 3 and result.body.ok == true, "command ok")
	expect(P.syncState.commandRunning == false, "busy flag cleared")
	-- A preview never counts as a sync.
	expect(P.syncState.lastSyncedRevision == nil)
end)

test("push remembers the revision; later local edits show as drift", function()
	resetSync()
	serve({
		["/api/health"] = health,
		["/api/tree"] = function()
			return reply({ revision = 7, tree = tree({ node("ReplicatedStorage", "ReplicatedStorage", {}) }) })
		end,
		["/api/backups/studio/create"] = function()
			return reply({ backup = { id = "b1", label = "b1" } })
		end,
		["/api/plugin/sync"] = syncReply(7),
	})
	P.runExclusive("push", P.pushLocalTree, false, false)
	expect(P.syncState.lastSyncedRevision == 7, "revision recorded")
	P.bridgeSyncOnce()
	local state = P.describeSyncState()
	expect(state == "insync", state)

	serve({
		["/api/plugin/sync"] = function()
			return reply({ project = { revision = 8, recentChanges = { "ReplicatedStorage/A.luau" } } })
		end,
	})
	P.bridgeSyncOnce()
	local localState, text = P.describeSyncState()
	expect(localState == "local", localState)
	expect(string.find(text, "ReplicatedStorage/A.luau", 1, true), text)
	expect(lastRequest("/api/plugin/sync").body.lastSyncedRevision == 7)
end)

test("Studio edits are counted, but not Nyjo's own edits or trash moves", function()
	resetSync()
	local RS = Mock.services.ReplicatedStorage
	-- During an operation (e.g. a push) nothing is counted...
	P.runExclusive("push", function()
		RS.DescendantAdded:Fire(Mock.newInstance("Part", "FromPush"))
	end)
	-- ...nor deferred signals right after it.
	RS.DescendantAdded:Fire(Mock.newInstance("Part", "Deferred"))
	expect(P.syncState.studioChanges == 0, "own edits ignored")

	Mock.nowMs += 1000
	local trash = Mock.newInstance("Folder", "NyjoTrash")
	local trashed = Mock.newInstance("Part", "Old")
	trashed.Parent = trash
	RS.DescendantAdded:Fire(trash)
	RS.DescendantAdded:Fire(trashed)
	expect(P.syncState.studioChanges == 0, "trash ignored")

	RS.DescendantAdded:Fire(Mock.newInstance("Part", "UserPart"))
	Mock.services.Workspace.DescendantRemoving:Fire(Mock.newInstance("Part", "Gone"))
	Mock.services.ScriptEditorService.TextDocumentDidChange:Fire({ IsCommandBar = function() return false end })
	Mock.services.ScriptEditorService.TextDocumentDidChange:Fire({ IsCommandBar = function() return true end })
	expect(P.syncState.studioChanges == 3, "counted " .. tostring(P.syncState.studioChanges))

	serve({ ["/api/plugin/sync"] = syncReply(9) })
	P.syncState.lastSyncedRevision = 9
	P.bridgeSyncOnce()
	expect(lastRequest("/api/plugin/sync").body.studioChangesSinceSync == 3)
	local state = P.describeSyncState()
	expect(state == "studio", state)
	P.syncState.lastSyncedRevision = 8
	state = P.describeSyncState()
	expect(state == "conflict", state)
end)

test("applied pull marks Studio and disk as in sync", function()
	resetSync()
	P.syncState.studioChanges = 4
	serve({
		["/api/health"] = health,
		["/api/sync-from-studio"] = function()
			return reply({ mode = "apply", applied = true, blocked = false, revision = 12, plan = { stats = {}, operations = {} } })
		end,
	})
	P.runExclusive("apply pull", P.syncFromStudio, "apply", false, false)
	expect(P.syncState.lastSyncedRevision == 12)
	expect(P.syncState.studioChanges == 0)

	-- A blocked pull changes nothing.
	P.syncState.studioChanges = 2
	serve({
		["/api/health"] = health,
		["/api/sync-from-studio"] = function()
			return reply({ mode = "apply", applied = false, blocked = true, revision = 13, plan = { stats = {}, operations = {} } })
		end,
	})
	P.runExclusive("apply pull", P.syncFromStudio, "apply", false, false)
	expect(P.syncState.lastSyncedRevision == 12 and P.syncState.studioChanges == 2)
end)

test("connection shows lost after 3s without a reply, quietly", function()
	resetSync()
	serve({ ["/api/plugin/sync"] = syncReply(1) })
	P.bridgeSyncOnce()
	expect((P.describeSyncState()) ~= "lost")
	P.statusBox.Text = "last push report"

	serve({
		["/api/plugin/sync"] = function()
			return { Success = false, StatusCode = 500, Body = "boom" }
		end,
	})
	Mock.nowMs += 2000
	expect(P.bridgeSyncOnce() == false)
	expect((P.describeSyncState()) ~= "lost", "2s is still fresh")
	Mock.nowMs += 1500
	P.bridgeSyncOnce()
	expect((P.describeSyncState()) == "lost", "3.5s is lost")
	P.refreshSyncLabel()
	expect(string.find(P.syncStatusLabel.Text, "lost", 1, true), P.syncStatusLabel.Text)
	expect(P.statusBox.Text == "last push report", "output box is not spammed")

	serve({ ["/api/plugin/sync"] = syncReply(1) })
	expect(P.bridgeSyncOnce() == true)
	P.refreshSyncLabel()
	expect(string.find(P.syncStatusLabel.Text, "Connected", 1, true), "reconnects on first reply")
end)

test("local parse errors are surfaced", function()
	resetSync()
	serve({
		["/api/plugin/sync"] = function()
			return reply({ project = { revision = 4, scanError = "bad meta.json" } })
		end,
	})
	P.bridgeSyncOnce()
	local state, text = P.describeSyncState()
	expect(state == "error" and string.find(text, "bad meta.json", 1, true), text)
end)

test("falls back to heartbeat + poll on servers without /api/plugin/sync", function()
	resetSync()
	serve({
		["/api/plugin/heartbeat"] = function()
			return reply({})
		end,
		["/api/plugin/poll"] = function()
			return reply({})
		end,
	})
	expect(P.bridgeSyncOnce() == true)
	expect(lastRequest("/api/plugin/poll") ~= nil)
	expect((P.describeSyncState()) ~= "lost")
end)
