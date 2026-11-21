--!nocheck
-- Minimal Roblox Studio mock: just enough of the engine API for the Nyjo
-- plugin's push/pull logic to run under the stand-alone `luau` CLI.

local M = {}

-------------------------------------------------------------------------------
-- JSON (HttpService:JSONEncode/JSONDecode)
-------------------------------------------------------------------------------
local function isArray(t)
	local n = 0
	for k in pairs(t) do
		if typeof(k) ~= "number" then
			return false
		end
		n += 1
	end
	return n == #t
end

local function encode(v)
	local tv = typeof(v)
	if v == nil then
		return "null"
	elseif tv == "boolean" then
		return tostring(v)
	elseif tv == "number" then
		if v ~= v or v == math.huge or v == -math.huge then
			error("cannot encode non-finite number")
		end
		return string.format("%.17g", v)
	elseif tv == "string" then
		return '"' .. v:gsub('[%c"\\]', function(c)
			return string.format("\\u%04x", string.byte(c))
		end) .. '"'
	elseif tv == "table" then
		local parts = {}
		if next(v) ~= nil and isArray(v) then
			for _, item in ipairs(v) do
				table.insert(parts, encode(item))
			end
			return "[" .. table.concat(parts, ",") .. "]"
		end
		if next(v) == nil then
			return "[]"
		end
		for k, item in pairs(v) do
			table.insert(parts, encode(tostring(k)) .. ":" .. encode(item))
		end
		table.sort(parts)
		return "{" .. table.concat(parts, ",") .. "}"
	end
	error("cannot encode " .. tv)
end

local function decode(str)
	local pos = 1
	local function ws()
		pos = string.find(str, "[^%s]", pos) or #str + 1
	end
	local value
	local function str_()
		local out = {}
		pos += 1
		while true do
			local c = string.sub(str, pos, pos)
			if c == '"' then
				pos += 1
				return table.concat(out)
			elseif c == "\\" then
				local n = string.sub(str, pos + 1, pos + 1)
				if n == "u" then
					table.insert(out, utf8.char(tonumber(string.sub(str, pos + 2, pos + 5), 16)))
					pos += 6
				else
					local map = { n = "\n", t = "\t", r = "\r", b = "\b", f = "\f" }
					table.insert(out, map[n] or n)
					pos += 2
				end
			else
				table.insert(out, c)
				pos += 1
			end
		end
	end
	function value()
		ws()
		local c = string.sub(str, pos, pos)
		if c == "{" then
			local t = {}
			pos += 1
			ws()
			if string.sub(str, pos, pos) == "}" then
				pos += 1
				return t
			end
			while true do
				ws()
				local k = str_()
				ws()
				pos += 1 -- :
				t[k] = value()
				ws()
				local d = string.sub(str, pos, pos)
				pos += 1
				if d == "}" then
					return t
				end
			end
		elseif c == "[" then
			local t = {}
			pos += 1
			ws()
			if string.sub(str, pos, pos) == "]" then
				pos += 1
				return t
			end
			while true do
				table.insert(t, value())
				ws()
				local d = string.sub(str, pos, pos)
				pos += 1
				if d == "]" then
					return t
				end
			end
		elseif c == '"' then
			return str_()
		elseif string.sub(str, pos, pos + 3) == "true" then
			pos += 4
			return true
		elseif string.sub(str, pos, pos + 4) == "false" then
			pos += 5
			return false
		elseif string.sub(str, pos, pos + 3) == "null" then
			pos += 4
			return nil
		else
			local s, e = string.find(str, "^-?[%d%.eE+-]+", pos)
			pos = e + 1
			return tonumber(string.sub(str, s, e))
		end
	end
	return value()
end
M.encode = encode
M.decode = decode

-------------------------------------------------------------------------------
-- Signals
-------------------------------------------------------------------------------
local function newSignal()
	local handlers = {}
	return {
		Connect = function(_, fn)
			table.insert(handlers, fn)
			return { Disconnect = function() end }
		end,
		Fire = function(_, ...)
			for _, fn in ipairs(handlers) do
				fn(...)
			end
		end,
	}
end

-------------------------------------------------------------------------------
-- Instances
-------------------------------------------------------------------------------
local SUPER = {
	Script = "LuaSourceContainer",
	LocalScript = "LuaSourceContainer",
	ModuleScript = "LuaSourceContainer",
	Part = "BasePart",
	MeshPart = "BasePart",
	TextLabel = "GuiObject",
	TextButton = "GuiObject",
	TextBox = "GuiObject",
	Frame = "GuiObject",
	StringValue = "ValueBase",
	IntValue = "ValueBase",
}

M.failNameSetFor = {}
-- Wall clock seen by the plugin via DateTime.now(); tests move it by hand.
M.nowMs = 1700000000000
-- When true, task.spawn runs the function immediately (the plugin's endless
-- heartbeat loops are started while this is still false).
M.runSpawned = false
local Methods = {}
local InstMT = {}

local function newInstance(className, name)
	local self = {
		__class = className,
		__props = { Name = name or className },
		__children = {},
		__attrs = {},
		__tags = {},
		__signals = {},
	}
	if SUPER[className] == "LuaSourceContainer" then
		self.__props.Source = ""
	end
	return setmetatable(self, InstMT)
end
M.newInstance = newInstance

InstMT.__index = function(t, k)
	if k == "ClassName" then
		return rawget(t, "__class")
	elseif k == "Parent" then
		return rawget(t, "__parent")
	end
	if Methods[k] then
		return Methods[k]
	end
	local v = t.__props[k]
	if v ~= nil then
		return v
	end
	if
		string.match(k, "Click$")
		or k == "FocusLost"
		or k == "Changed"
		or k == "Unloading"
		or k == "DescendantAdded"
		or k == "DescendantRemoving"
		or k == "TextDocumentDidChange"
		or k == "OnUndo"
		or k == "OnRedo"
	then
		t.__signals[k] = t.__signals[k] or newSignal()
		return t.__signals[k]
	end
	return nil
end

InstMT.__newindex = function(t, k, v)
	if k == "Parent" then
		local old = rawget(t, "__parent")
		if old then
			for i, c in ipairs(old.__children) do
				if c == t then
					table.remove(old.__children, i)
					break
				end
			end
		end
		rawset(t, "__parent", v)
		if v then
			table.insert(v.__children, t)
		end
		return
	end
	if k == "Name" and M.failNameSetFor[v] then
		error("mock: refusing to set Name to " .. tostring(v))
	end
	t.__props[k] = v
end

InstMT.__tostring = function(t)
	return t.__props.Name
end

function Methods.GetChildren(self)
	return table.clone(self.__children)
end
function Methods.GetDescendants(self)
	local out = {}
	local function walk(n)
		for _, c in ipairs(n.__children) do
			table.insert(out, c)
			walk(c)
		end
	end
	walk(self)
	return out
end
function Methods.FindFirstChild(self, name)
	for _, c in ipairs(self.__children) do
		if c.Name == name then
			return c
		end
	end
	return nil
end
function Methods.IsA(self, className)
	local c = self.__class
	while c do
		if c == className then
			return true
		end
		c = SUPER[c]
	end
	return className == "Instance"
end
function Methods.GetAttribute(self, name)
	return self.__attrs[name]
end
function Methods.SetAttribute(self, name, value)
	self.__attrs[name] = value
end
function Methods.GetAttributes(self)
	return table.clone(self.__attrs)
end
function Methods.GetFullName(self)
	local parts = {}
	local n = self
	while n do
		table.insert(parts, 1, n.Name)
		n = rawget(n, "__parent")
	end
	return table.concat(parts, ".")
end
function Methods.Destroy(self)
	self.Parent = nil
	rawset(self, "__destroyed", true)
end
function Methods.GetPropertyChangedSignal(self, prop)
	self.__signals["prop:" .. prop] = self.__signals["prop:" .. prop] or newSignal()
	return self.__signals["prop:" .. prop]
end
function Methods.SetActive() end
function Methods.FindFirstAncestor(self, name)
	local n = rawget(self, "__parent")
	while n do
		if n.Name == name then
			return n
		end
		n = rawget(n, "__parent")
	end
	return nil
end

-------------------------------------------------------------------------------
-- Environment
-------------------------------------------------------------------------------
function M.install(env)
	local dataModel = newInstance("DataModel", "Game")
	dataModel.__props.PlaceId = 0
	local services = {}
	M.services = services
	M.recordings = {}
	M.httpHandler = function()
		return { Success = false, StatusCode = 503, Body = "no server" }
	end
	M.running = false

	local function service(name)
		if not services[name] then
			local s = newInstance(name, name)
			s.Parent = dataModel
			services[name] = s
		end
		return services[name]
	end

	local chs = service("ChangeHistoryService")
	chs.__props.TryBeginRecording = function(_, label)
		local id = "rec" .. tostring(#M.recordings + 1)
		table.insert(M.recordings, { id = id, label = label })
		return id
	end
	chs.__props.FinishRecording = function(_, id, op)
		for _, r in ipairs(M.recordings) do
			if r.id == id then
				r.finished = tostring(op)
			end
		end
	end
	local cs = service("CollectionService")
	cs.__props.GetTags = function(_, inst)
		local out = {}
		for tag in pairs(inst.__tags) do
			table.insert(out, tag)
		end
		table.sort(out)
		return out
	end
	cs.__props.HasTag = function(_, inst, tag)
		return inst.__tags[tag] == true
	end
	cs.__props.AddTag = function(_, inst, tag)
		inst.__tags[tag] = true
	end
	cs.__props.RemoveTag = function(_, inst, tag)
		inst.__tags[tag] = nil
	end
	local http = service("HttpService")
	local guid = 0
	http.__props.GenerateGUID = function()
		guid += 1
		return string.format("00000000-0000-0000-0000-%012d", guid)
	end
	http.__props.JSONEncode = function(_, v)
		return encode(v)
	end
	http.__props.JSONDecode = function(_, s)
		return decode(s)
	end
	http.__props.RequestAsync = function(_, req)
		return M.httpHandler(req)
	end
	service("MarketplaceService").__props.GetProductInfoAsync = function()
		return { Name = "Mock Place" }
	end
	service("RunService").__props.IsRunning = function()
		return M.running
	end
	service("ScriptEditorService").__props.UpdateSourceAsync = function(_, inst, fn)
		inst.__props.Source = fn(inst.__props.Source)
	end
	for _, name in ipairs({ "Workspace", "ServerStorage", "ReplicatedStorage", "ServerScriptService", "StarterGui" }) do
		service(name)
	end

	dataModel.__props.GetService = function(_, name)
		return service(name)
	end
	env.game = dataModel

	local function enumProxy(path)
		return setmetatable({}, {
			__index = function(_, k)
				local item = enumProxy(path .. "." .. k)
				rawset(item, "Name", k)
				return item
			end,
			__tostring = function()
				return path
			end,
		})
	end
	env.Enum = enumProxy("Enum")

	local function ctor(kind)
		return setmetatable({}, {
			__index = function()
				return function(...)
					return { __kind = kind, ... }
				end
			end,
		})
	end
	for _, kind in ipairs({ "UDim", "UDim2", "Color3", "Vector2", "Vector3", "CFrame", "DockWidgetPluginGuiInfo", "Font" }) do
		env[kind] = ctor(kind)
	end
	env.DateTime = {
		now = function()
			return {
				UnixTimestampMillis = M.nowMs,
				FormatUniversalTime = function()
					return "20261121T120000Z"
				end,
			}
		end,
	}
	env.Instance = {
		new = function(className)
			if className == "Unknownium" then
				error("mock: unable to create " .. className)
			end
			return newInstance(className)
		end,
	}
	env.task = {
		spawn = function(fn, ...)
			if M.runSpawned then
				fn(...)
			end
		end,
		wait = function() end,
	}
	env.warn = function() end

	local settings = {}
	local pluginObj = newInstance("Plugin", "Nyjo")
	pluginObj.__props.CreateToolbar = function()
		return {
			CreateButton = function()
				return newInstance("PluginToolbarButton")
			end,
		}
	end
	pluginObj.__props.CreateDockWidgetPluginGuiAsync = function()
		return newInstance("DockWidgetPluginGui")
	end
	pluginObj.__props.GetSetting = function(_, k)
		return settings[k]
	end
	pluginObj.__props.SetSetting = function(_, k, v)
		settings[k] = v
	end
	env.plugin = pluginObj
end

return M
