-- Nyjo Studio bridge plugin (installed by `nyjo install`).
-- Talks only to the local nyjo server on 127.0.0.1.
local ChangeHistoryService = game:GetService("ChangeHistoryService")
local CollectionService = game:GetService("CollectionService")
local HttpService = game:GetService("HttpService")
local MarketplaceService = game:GetService("MarketplaceService")
local RunService = game:GetService("RunService")
local ScriptEditorService = game:GetService("ScriptEditorService")
local ServerStorage = game:GetService("ServerStorage")

local TOOLBAR_ID = "NyjoToolbar"
local BUTTON_ID = "NyjoToggle"
local WIDGET_ID = "NyjoWidget"
local SETTINGS_KEY_PORT = "nyjo_port"
local SETTINGS_KEY_LAST_STUDIO_BACKUP_ID = "nyjo_last_studio_backup_id"
local SETTINGS_KEY_LAST_STUDIO_BACKUP_PLACE_ID = "nyjo_last_studio_backup_place_id"
local SETTINGS_KEY_LAST_STUDIO_BACKUP_LABEL = "nyjo_last_studio_backup_label"
local DEFAULT_PORT = 34872
local BRIDGE_VERSION = "nyjo-studio-bridge/2"
-- Even with deletes allowed, refuse a push that would remove more than this
-- many instances (counting descendants). Protects against a broken or
-- half-empty local tree wiping the place.
local MAX_PUSH_DELETED_INSTANCES = 200
local TRASH_FOLDER_NAME = "NyjoTrash"
local POLL_INTERVAL_SECONDS = 1
local MAX_POLL_BACKOFF_SECONDS = 15
-- Properties Nyjo never writes from local data: identity is name + class and
-- the tree structure, not a property map.
local PROTECTED_PROPERTIES = {
	Name = true,
	Parent = true,
	ClassName = true,
	Archivable = true,
}
local BRIDGE_SESSION_ID = HttpService:GenerateGUID(false)
local MANAGED_ATTRIBUTE = "NyjoManagedBy"
local MANAGED_CHILD_KEYS_ATTRIBUTE = "NyjoManagedChildKeys"
local MANAGED_VALUE = "nyjo"
local PANEL_COLOR = Color3.fromRGB(28, 30, 36)
local BORDER_COLOR = Color3.fromRGB(78, 82, 94)
local OUTPUT_BG = Color3.fromRGB(20, 22, 28)
local PRIMARY_TEXT = Color3.fromRGB(245, 247, 250)
local SECONDARY_TEXT = Color3.fromRGB(214, 219, 230)
local MUTED_TEXT = Color3.fromRGB(170, 176, 188)
local BUTTON_BG = Color3.fromRGB(232, 235, 241)
local BUTTON_TEXT = Color3.fromRGB(24, 26, 31)
local INPUT_BG = Color3.fromRGB(236, 239, 244)
local INPUT_TEXT = Color3.fromRGB(28, 30, 36)

local ROOT_SERVICE_CLASSES = {
	Lighting = true,
	ReplicatedFirst = true,
	ReplicatedStorage = true,
	ServerScriptService = true,
	ServerStorage = true,
	SoundService = true,
	StarterGui = true,
	StarterPlayer = true,
	Teams = true,
	TextChatService = true,
	Workspace = true,
}

local UNSAFE_CREATE_CLASS_REASONS = {
	IntersectOperation = "CSG operation geometry does not round-trip safely through Nyjo yet",
	NegateOperation = "CSG operation geometry does not round-trip safely through Nyjo yet",
	Terrain = "terrain voxel data does not round-trip safely through Nyjo yet",
	UnionOperation = "CSG operation geometry does not round-trip safely through Nyjo yet",
}

local BASE_PART_PROPERTIES = {
	"Anchored",
	"BottomSurface",
	"CanCollide",
	"CanQuery",
	"CanTouch",
	"CastShadow",
	"CFrame",
	"CollisionGroup",
	"Color",
	"Locked",
	"Massless",
	"Material",
	"MaterialVariant",
	"PivotOffset",
	"Reflectance",
	"Shape",
	"Size",
	"TopSurface",
	"Transparency",
}
local MESH_PART_PROPERTIES = { "DoubleSided", "MeshId", "RenderFidelity", "TextureID" }
local DATA_MODEL_MESH_PROPERTIES = { "Offset", "Scale", "VertexColor" }
local SPECIAL_MESH_PROPERTIES = { "MeshId", "MeshType", "TextureId" }
local SURFACE_APPEARANCE_PROPERTIES = { "AlphaMode", "ColorMap", "MetalnessMap", "NormalMap", "RoughnessMap" }
local VALUE_BASE_PROPERTIES = { "Value" }
local SCREEN_GUI_PROPERTIES = { "DisplayOrder", "IgnoreGuiInset", "ResetOnSpawn" }
local GUI_OBJECT_PROPERTIES = {
	"AnchorPoint",
	"AutomaticSize",
	"BackgroundColor3",
	"BackgroundTransparency",
	"BorderColor3",
	"BorderSizePixel",
	"ClipsDescendants",
	"LayoutOrder",
	"Position",
	"Rotation",
	"Size",
	"SizeConstraint",
	"Visible",
	"ZIndex",
}
local TEXT_PROPERTIES = {
	"Font",
	"FontFace",
	"LineHeight",
	"RichText",
	"Text",
	"TextColor3",
	"TextScaled",
	"TextSize",
	"TextTransparency",
	"TextWrapped",
	"TextXAlignment",
	"TextYAlignment",
}
local BUTTON_PROPERTIES = { "AutoButtonColor", "Modal", "Selected" }
local IMAGE_PROPERTIES = { "Image", "ImageColor3", "ImageTransparency", "ScaleType" }
local SCROLLING_FRAME_PROPERTIES = {
	"AutomaticCanvasSize",
	"CanvasPosition",
	"CanvasSize",
	"ScrollBarThickness",
	"ScrollingDirection",
}
local UI_LIST_LAYOUT_PROPERTIES = {
	"FillDirection",
	"HorizontalAlignment",
	"Padding",
	"SortOrder",
	"VerticalAlignment",
}
local UI_GRID_LAYOUT_PROPERTIES = {
	"CellPadding",
	"CellSize",
	"FillDirection",
	"HorizontalAlignment",
	"SortOrder",
	"VerticalAlignment",
}
local UI_PADDING_PROPERTIES = { "PaddingTop", "PaddingBottom", "PaddingLeft", "PaddingRight" }
local UI_CORNER_SHORTHAND_PROPERTIES = { "CornerRadius" }
local UI_CORNER_INDIVIDUAL_PROPERTIES = {
	"TopLeftRadius",
	"TopRightRadius",
	"BottomRightRadius",
	"BottomLeftRadius",
}
local UI_CORNER_INDIVIDUAL_PROPERTY_SET = {
	TopLeftRadius = true,
	TopRightRadius = true,
	BottomRightRadius = true,
	BottomLeftRadius = true,
}
local UI_STROKE_PROPERTIES = { "ApplyStrokeMode", "Color", "Thickness", "Transparency" }
local UI_SHADOW_PROPERTIES = {
	"BlurRadius",
	"Color",
	"Enabled",
	"Offset",
	"Spread",
	"Transparency",
	"ZIndex",
}
local DECAL_PROPERTIES = { "Color3", "Face", "Texture", "Transparency" }
local TEXTURE_PROPERTIES = { "OffsetStudsU", "OffsetStudsV", "StudsPerTileU", "StudsPerTileV" }

local toolbar = plugin:CreateToolbar(TOOLBAR_ID)
local toggleButton = toolbar:CreateButton(BUTTON_ID, "Open Nyjo", "")
toggleButton.ClickableWhenViewportHidden = true

local widgetInfo = DockWidgetPluginGuiInfo.new(
	Enum.InitialDockState.Right,
	true,
	false,
	360,
	392,
	280,
	240
)

-- CreateDockWidgetPluginGui is deprecated in favour of the Async variant;
-- fall back for older Studio builds.
local widget
do
	local ok, created = pcall(function()
		return plugin:CreateDockWidgetPluginGuiAsync(WIDGET_ID, widgetInfo)
	end)
	if ok and created then
		widget = created
	else
		widget = plugin:CreateDockWidgetPluginGui(WIDGET_ID, widgetInfo)
	end
end
widget.Title = "Nyjo"

local root = Instance.new("Frame")
root.Name = "Root"
root.Size = UDim2.fromScale(1, 1)
root.BackgroundColor3 = PANEL_COLOR
root.BorderSizePixel = 0
root.Parent = widget

local padding = Instance.new("UIPadding")
padding.PaddingTop = UDim.new(0, 12)
padding.PaddingBottom = UDim.new(0, 12)
padding.PaddingLeft = UDim.new(0, 12)
padding.PaddingRight = UDim.new(0, 12)
padding.Parent = root

local layout = Instance.new("UIListLayout")
layout.FillDirection = Enum.FillDirection.Vertical
layout.HorizontalAlignment = Enum.HorizontalAlignment.Left
layout.Padding = UDim.new(0, 8)
layout.Parent = root

local title = Instance.new("TextLabel")
title.Name = "Title"
title.LayoutOrder = 1
title.BackgroundTransparency = 1
title.Size = UDim2.new(1, 0, 0, 22)
title.Font = Enum.Font.SourceSansBold
title.TextSize = 20
title.TextColor3 = PRIMARY_TEXT
title.TextXAlignment = Enum.TextXAlignment.Left
title.Text = "Nyjo Bridge"
title.Parent = root

local description = Instance.new("TextLabel")
description.Name = "Description"
description.LayoutOrder = 2
description.BackgroundTransparency = 1
description.Size = UDim2.new(1, 0, 0, 34)
description.Font = Enum.Font.SourceSans
description.TextSize = 16
description.TextColor3 = SECONDARY_TEXT
description.TextWrapped = true
description.TextXAlignment = Enum.TextXAlignment.Left
description.TextYAlignment = Enum.TextYAlignment.Top
description.Text = "Nyjo now uses a browser dashboard as the main control room. This Studio widget stays open as the bridge and still offers local fallback buttons."
description.Parent = root

local portLabel = Instance.new("TextLabel")
portLabel.Name = "PortLabel"
portLabel.LayoutOrder = 3
portLabel.BackgroundTransparency = 1
portLabel.Size = UDim2.new(1, 0, 0, 18)
portLabel.Font = Enum.Font.SourceSansSemibold
portLabel.TextSize = 16
portLabel.TextColor3 = PRIMARY_TEXT
portLabel.TextXAlignment = Enum.TextXAlignment.Left
portLabel.Text = "Server Port"
portLabel.Parent = root

local portBox = Instance.new("TextBox")
portBox.Name = "PortBox"
portBox.LayoutOrder = 4
portBox.Size = UDim2.new(1, 0, 0, 30)
portBox.BackgroundColor3 = INPUT_BG
portBox.BorderColor3 = BORDER_COLOR
portBox.ClearTextOnFocus = false
portBox.Font = Enum.Font.Code
portBox.PlaceholderText = tostring(DEFAULT_PORT)
portBox.PlaceholderColor3 = MUTED_TEXT
portBox.TextColor3 = INPUT_TEXT
portBox.TextSize = 16
portBox.Parent = root

local buttonRow = Instance.new("Frame")
buttonRow.Name = "Buttons"
buttonRow.LayoutOrder = 5
buttonRow.BackgroundTransparency = 1
buttonRow.Size = UDim2.new(1, 0, 0, 32)
buttonRow.Parent = root

local buttonLayout = Instance.new("UIListLayout")
buttonLayout.FillDirection = Enum.FillDirection.Horizontal
buttonLayout.HorizontalAlignment = Enum.HorizontalAlignment.Left
buttonLayout.Padding = UDim.new(0, 8)
buttonLayout.Parent = buttonRow

local checkButton = Instance.new("TextButton")
checkButton.Name = "CheckServer"
checkButton.Size = UDim2.new(0, 120, 1, 0)
checkButton.BackgroundColor3 = BUTTON_BG
checkButton.BorderColor3 = BORDER_COLOR
checkButton.Font = Enum.Font.SourceSansSemibold
checkButton.TextColor3 = BUTTON_TEXT
checkButton.TextSize = 16
checkButton.Text = "Check Server"
checkButton.Parent = buttonRow

local treeButton = Instance.new("TextButton")
treeButton.Name = "FetchTree"
treeButton.Size = UDim2.new(0, 120, 1, 0)
treeButton.BackgroundColor3 = BUTTON_BG
treeButton.BorderColor3 = BORDER_COLOR
treeButton.Font = Enum.Font.SourceSansSemibold
treeButton.TextColor3 = BUTTON_TEXT
treeButton.TextSize = 16
treeButton.Text = "Preview Tree"
treeButton.Parent = buttonRow

local pushButton = Instance.new("TextButton")
pushButton.Name = "PushTree"
pushButton.Size = UDim2.new(0, 120, 1, 0)
pushButton.BackgroundColor3 = BUTTON_BG
pushButton.BorderColor3 = BORDER_COLOR
pushButton.Font = Enum.Font.SourceSansSemibold
pushButton.TextColor3 = BUTTON_TEXT
pushButton.TextSize = 16
pushButton.Text = "Sync To Studio"
pushButton.Parent = buttonRow

local syncRow = Instance.new("Frame")
syncRow.Name = "SyncButtons"
syncRow.LayoutOrder = 6
syncRow.BackgroundTransparency = 1
syncRow.Size = UDim2.new(1, 0, 0, 32)
syncRow.Parent = root

local syncLayout = Instance.new("UIListLayout")
syncLayout.FillDirection = Enum.FillDirection.Horizontal
syncLayout.HorizontalAlignment = Enum.HorizontalAlignment.Left
syncLayout.Padding = UDim.new(0, 8)
syncLayout.Parent = syncRow

local previewPullButton = Instance.new("TextButton")
previewPullButton.Name = "PreviewPull"
previewPullButton.Size = UDim2.new(0, 110, 1, 0)
previewPullButton.BackgroundColor3 = BUTTON_BG
previewPullButton.BorderColor3 = BORDER_COLOR
previewPullButton.Font = Enum.Font.SourceSansSemibold
previewPullButton.TextColor3 = BUTTON_TEXT
previewPullButton.TextSize = 16
previewPullButton.Text = "Preview Pull"
previewPullButton.Parent = syncRow

local applyPullButton = Instance.new("TextButton")
applyPullButton.Name = "ApplyPull"
applyPullButton.Size = UDim2.new(0, 110, 1, 0)
applyPullButton.BackgroundColor3 = BUTTON_BG
applyPullButton.BorderColor3 = BORDER_COLOR
applyPullButton.Font = Enum.Font.SourceSansSemibold
applyPullButton.TextColor3 = BUTTON_TEXT
applyPullButton.TextSize = 16
applyPullButton.Text = "Apply Pull"
applyPullButton.Parent = syncRow

local forcePullButton = Instance.new("TextButton")
forcePullButton.Name = "ForcePull"
forcePullButton.Size = UDim2.new(0, 110, 1, 0)
forcePullButton.BackgroundColor3 = BUTTON_BG
forcePullButton.BorderColor3 = BORDER_COLOR
forcePullButton.Font = Enum.Font.SourceSansSemibold
forcePullButton.TextColor3 = BUTTON_TEXT
forcePullButton.TextSize = 16
forcePullButton.Text = "Force Pull"
forcePullButton.Parent = syncRow

local safetyRow = Instance.new("Frame")
safetyRow.Name = "SafetyButtons"
safetyRow.LayoutOrder = 7
safetyRow.BackgroundTransparency = 1
safetyRow.Size = UDim2.new(1, 0, 0, 32)
safetyRow.Parent = root

local safetyLayout = Instance.new("UIListLayout")
safetyLayout.FillDirection = Enum.FillDirection.Horizontal
safetyLayout.HorizontalAlignment = Enum.HorizontalAlignment.Left
safetyLayout.Padding = UDim.new(0, 8)
safetyLayout.Parent = safetyRow

local restoreStudioBackupButton = Instance.new("TextButton")
restoreStudioBackupButton.Name = "RestoreStudioBackup"
restoreStudioBackupButton.Size = UDim2.new(0, 180, 1, 0)
restoreStudioBackupButton.BackgroundColor3 = BUTTON_BG
restoreStudioBackupButton.BorderColor3 = BORDER_COLOR
restoreStudioBackupButton.Font = Enum.Font.SourceSansSemibold
restoreStudioBackupButton.TextColor3 = BUTTON_TEXT
restoreStudioBackupButton.TextSize = 16
restoreStudioBackupButton.Text = "Restore Studio Backup"
restoreStudioBackupButton.Parent = safetyRow

local status = Instance.new("TextBox")
status.Name = "Status"
status.LayoutOrder = 8
status.MultiLine = true
status.ClearTextOnFocus = false
status.TextEditable = false
status.BackgroundColor3 = OUTPUT_BG
status.BorderColor3 = BORDER_COLOR
status.Size = UDim2.new(1, 0, 0, 208)
status.Font = Enum.Font.Code
status.TextColor3 = PRIMARY_TEXT
status.TextSize = 15
status.TextWrapped = false
status.TextXAlignment = Enum.TextXAlignment.Left
status.TextYAlignment = Enum.TextYAlignment.Top
status.Text = "Waiting for nyjo..."
status.Parent = root

local function readPort()
	local saved = plugin:GetSetting(SETTINGS_KEY_PORT)
	if typeof(saved) == "number" and saved > 0 then
		return saved
	end

	local parsed = tonumber(portBox.Text)
	if parsed and parsed > 0 then
		return parsed
	end

	return DEFAULT_PORT
end

local function savePort()
	local parsed = tonumber(portBox.Text)
	if parsed and parsed > 0 then
		plugin:SetSetting(SETTINGS_KEY_PORT, parsed)
		return parsed
	end

	portBox.Text = tostring(readPort())
	return readPort()
end

local function setStatus(message)
	status.Text = message
end

local function dashboardUrl()
	return ("http://127.0.0.1:%d/"):format(readPort())
end

setStatus("Nyjo Studio bridge ready.\n\nUse the web dashboard at " .. dashboardUrl() .. " to inspect logs, queue Studio work, and review pull plans.\n\nSafety: a push saves a Studio backup first, is one Ctrl+Z step, rolls itself back on error, and never removes anything unless deletes are explicitly allowed (removed instances go to ServerStorage.NyjoTrash).")

local function request(path, method, body)
	local port = savePort()
	local url = ("http://127.0.0.1:%d%s"):format(port, path)
	local requestBody = nil
	if body ~= nil then
		requestBody = HttpService:JSONEncode(body)
	end

	return HttpService:RequestAsync({
		Url = url,
		Method = method or "GET",
		Body = requestBody,
		Headers = {
			["Content-Type"] = "application/json",
			["Accept"] = "application/json",
		},
	})
end

local function fetchJson(path)
	local response = request(path, "GET", nil)
	if not response.Success then
		error("HTTP " .. tostring(response.StatusCode) .. "\n" .. tostring(response.Body))
	end

	return HttpService:JSONDecode(response.Body)
end

local function postJson(path, body)
	local response = request(path, "POST", body)
	if not response.Success then
		error("HTTP " .. tostring(response.StatusCode) .. "\n" .. tostring(response.Body))
	end

	return HttpService:JSONDecode(response.Body)
end

local function tryPostJson(path, body)
	local ok, result = pcall(function()
		return postJson(path, body)
	end)
	return ok, result
end

local function countNodes(node)
	local total = 1
	local children = node.children
	if typeof(children) == "table" then
		for _, child in ipairs(children) do
			total = total + countNodes(child)
		end
	end
	return total
end

local function appendTreeLines(node, depth, lines, limit)
	if #lines >= limit then
		return
	end

	local indent = string.rep("  ", depth)
	local className = tostring(node.className or "Unknown")
	local name = tostring(node.name or "Unknown")
	table.insert(lines, string.format("%s- %s [%s]", indent, name, className))

	local children = node.children
	if typeof(children) ~= "table" then
		return
	end

	for _, child in ipairs(children) do
		if #lines >= limit then
			return
		end
		appendTreeLines(child, depth + 1, lines, limit)
	end
end

local function buildTreePreview(tree, limit)
	local lines = {}
	appendTreeLines(tree, 0, lines, limit)

	if countNodes(tree) > #lines then
		table.insert(lines, "...")
	end

	return table.concat(lines, "\n")
end

local function formatStringArray(values, limit)
	if typeof(values) ~= "table" then
		return "(none)"
	end

	local lines = {}
	local count = 0
	for _, value in ipairs(values) do
		if typeof(value) == "string" then
			count = count + 1
			if count <= limit then
				table.insert(lines, "- " .. value)
			end
		end
	end

	if count == 0 then
		return "(none)"
	end

	if count > limit then
		table.insert(lines, "...")
	end

	return table.concat(lines, "\n")
end

local function formatOperationLines(operations, limit)
	if typeof(operations) ~= "table" or #operations == 0 then
		return "(no changed paths)"
	end

	local lines = {}
	local capped = math.min(#operations, limit)
	for index = 1, capped do
		local operation = operations[index]
		table.insert(
			lines,
			string.format(
				"[%s %s] %s",
				tostring(operation.action or "?"),
				tostring(operation.kind or "?"),
				tostring(operation.path or "?")
			)
		)
	end

	if #operations > capped then
		table.insert(lines, "...")
	end

	return table.concat(lines, "\n")
end

local function formatConflictLines(conflicts, limit)
	if typeof(conflicts) ~= "table" or #conflicts == 0 then
		return "(no conflicts)"
	end

	local lines = {}
	local capped = math.min(#conflicts, limit)
	for index = 1, capped do
		local conflict = conflicts[index]
		table.insert(
			lines,
			string.format(
				"- %s: %s",
				tostring(conflict.path or "?"),
				tostring(conflict.message or "conflict")
			)
		)
	end

	if #conflicts > capped then
		table.insert(lines, "...")
	end

	return table.concat(lines, "\n")
end

local function countStudioInstanceDescendants(instance)
	local total = 1
	for _, child in ipairs(instance:GetChildren()) do
		total = total + countStudioInstanceDescendants(child)
	end
	return total
end

local function countSnapshotDescendants(node)
	local total = 1
	local children = node.children
	if typeof(children) ~= "table" then
		return total
	end

	for _, child in ipairs(children) do
		total = total + countSnapshotDescendants(child)
	end

	return total
end

local function encodeTypedValue(value)
	local valueType = typeof(value)
	if valueType == "number" then
		-- JSON cannot carry NaN/inf; dropping the value beats failing the snapshot.
		if value ~= value or value == math.huge or value == -math.huge then
			return nil
		end
		return value
	end
	if valueType == "boolean" or valueType == "string" then
		return value
	end

	if valueType == "Font" then
		return {
			__nyjoType = "Font",
			family = value.Family,
			weight = value.Weight.Name,
			style = value.Style.Name,
		}
	end

	if valueType == "Color3" then
		return {
			__nyjoType = "Color3",
			r = value.R,
			g = value.G,
			b = value.B,
		}
	end

	if valueType == "Vector2" then
		return {
			__nyjoType = "Vector2",
			x = value.X,
			y = value.Y,
		}
	end

	if valueType == "Vector3" then
		return {
			__nyjoType = "Vector3",
			x = value.X,
			y = value.Y,
			z = value.Z,
		}
	end

	if valueType == "CFrame" then
		return {
			__nyjoType = "CFrame",
			components = { value:GetComponents() },
		}
	end

	if valueType == "UDim" then
		return {
			__nyjoType = "UDim",
			scale = value.Scale,
			offset = value.Offset,
		}
	end

	if valueType == "UDim2" then
		return {
			__nyjoType = "UDim2",
			x = {
				scale = value.X.Scale,
				offset = value.X.Offset,
			},
			y = {
				scale = value.Y.Scale,
				offset = value.Y.Offset,
			},
		}
	end

	if valueType == "EnumItem" then
		local enumTypeName, enumValueName = string.match(tostring(value), "^Enum%.([^.]+)%.(.+)$")
		if enumTypeName == nil or enumValueName == nil then
			return nil
		end
		return {
			__nyjoType = "EnumItem",
			enumType = enumTypeName,
			value = enumValueName,
		}
	end

	return nil
end

local function decodeTypedValue(value)
	if typeof(value) ~= "table" then
		return value
	end

	local marker = value.__nyjoType
	if marker == "Font" then
		if typeof(value.family) ~= "string" then
			return nil
		end
		local ok, font = pcall(function()
			return Font.new(
				value.family,
				Enum.FontWeight[value.weight or "Regular"],
				Enum.FontStyle[value.style or "Normal"]
			)
		end)
		return ok and font or nil
	end

	if marker == "Color3" then
		return Color3.new(tonumber(value.r) or 0, tonumber(value.g) or 0, tonumber(value.b) or 0)
	end

	if marker == "Vector2" then
		return Vector2.new(tonumber(value.x) or 0, tonumber(value.y) or 0)
	end

	if marker == "Vector3" then
		return Vector3.new(tonumber(value.x) or 0, tonumber(value.y) or 0, tonumber(value.z) or 0)
	end

	if marker == "CFrame" then
		local components = value.components
		if typeof(components) ~= "table" or #components < 12 then
			return nil
		end
		return CFrame.new(
			tonumber(components[1]) or 0,
			tonumber(components[2]) or 0,
			tonumber(components[3]) or 0,
			tonumber(components[4]) or 1,
			tonumber(components[5]) or 0,
			tonumber(components[6]) or 0,
			tonumber(components[7]) or 0,
			tonumber(components[8]) or 1,
			tonumber(components[9]) or 0,
			tonumber(components[10]) or 0,
			tonumber(components[11]) or 0,
			tonumber(components[12]) or 1
		)
	end

	if marker == "UDim" then
		return UDim.new(tonumber(value.scale) or 0, tonumber(value.offset) or 0)
	end

	if marker == "UDim2" then
		local x = typeof(value.x) == "table" and value.x or {}
		local y = typeof(value.y) == "table" and value.y or {}
		return UDim2.new(
			tonumber(x.scale) or 0,
			tonumber(x.offset) or 0,
			tonumber(y.scale) or 0,
			tonumber(y.offset) or 0
		)
	end

	if marker == "EnumItem" then
		local enumTypeName = value.enumType
		local enumValueName = value.value
		if typeof(enumTypeName) ~= "string" or typeof(enumValueName) ~= "string" then
			return nil
		end

		local enumType = Enum[enumTypeName]
		if enumType == nil then
			return nil
		end

		local ok, enumItem = pcall(function()
			return enumType[enumValueName]
		end)
		if ok then
			return enumItem
		end
		return nil
	end

	return nil
end

local function copyProperties(instance, propertyNames, destination)
	for _, propertyName in ipairs(propertyNames) do
		local ok, value = pcall(function()
			return instance[propertyName]
		end)
		if ok then
			local encoded = encodeTypedValue(value)
			if encoded ~= nil then
				destination[propertyName] = encoded
			end
		end
	end
end

local function copyUICornerProperties(instance, destination)
	copyProperties(instance, UI_CORNER_INDIVIDUAL_PROPERTIES, destination)

	for _, propertyName in ipairs(UI_CORNER_INDIVIDUAL_PROPERTIES) do
		if destination[propertyName] ~= nil then
			return
		end
	end

	copyProperties(instance, UI_CORNER_SHORTHAND_PROPERTIES, destination)
end

local function extractStudioProperties(instance)
	local properties = {}

	if instance:IsA("BasePart") then
		copyProperties(instance, BASE_PART_PROPERTIES, properties)
	end

	if instance:IsA("MeshPart") then
		copyProperties(instance, MESH_PART_PROPERTIES, properties)
	end

	if instance:IsA("ValueBase") then
		copyProperties(instance, VALUE_BASE_PROPERTIES, properties)
	end

	if instance:IsA("ScreenGui") then
		copyProperties(instance, SCREEN_GUI_PROPERTIES, properties)
	end

	if instance:IsA("GuiObject") then
		copyProperties(instance, GUI_OBJECT_PROPERTIES, properties)
	end

	if instance:IsA("TextLabel") or instance:IsA("TextButton") or instance:IsA("TextBox") then
		copyProperties(instance, TEXT_PROPERTIES, properties)
	end

	if instance:IsA("TextButton") or instance:IsA("ImageButton") then
		copyProperties(instance, BUTTON_PROPERTIES, properties)
	end

	if instance:IsA("ImageLabel") or instance:IsA("ImageButton") then
		copyProperties(instance, IMAGE_PROPERTIES, properties)
	end

	if instance:IsA("ScrollingFrame") then
		copyProperties(instance, SCROLLING_FRAME_PROPERTIES, properties)
	end

	if instance:IsA("UIListLayout") then
		copyProperties(instance, UI_LIST_LAYOUT_PROPERTIES, properties)
	end

	if instance:IsA("UIGridLayout") then
		copyProperties(instance, UI_GRID_LAYOUT_PROPERTIES, properties)
	end

	if instance:IsA("UIPadding") then
		copyProperties(instance, UI_PADDING_PROPERTIES, properties)
	end

	if instance:IsA("UICorner") then
		copyUICornerProperties(instance, properties)
	end

	if instance:IsA("UIStroke") then
		copyProperties(instance, UI_STROKE_PROPERTIES, properties)
	end

	if instance:IsA("UIShadow") then
		copyProperties(instance, UI_SHADOW_PROPERTIES, properties)
	end

	if instance:IsA("Decal") or instance:IsA("Texture") then
		copyProperties(instance, DECAL_PROPERTIES, properties)
	end

	if instance:IsA("Texture") then
		copyProperties(instance, TEXTURE_PROPERTIES, properties)
	end

	if instance:IsA("DataModelMesh") then
		copyProperties(instance, DATA_MODEL_MESH_PROPERTIES, properties)
	end

	if instance:IsA("SpecialMesh") then
		copyProperties(instance, SPECIAL_MESH_PROPERTIES, properties)
	end

	if instance:IsA("SurfaceAppearance") then
		copyProperties(instance, SURFACE_APPEARANCE_PROPERTIES, properties)
	end

	return properties
end

local function emptyObject()
	return {}
end

local function normalizeDictionary(value)
	if typeof(value) ~= "table" then
		return emptyObject()
	end

	local result = {}
	for key, entry in pairs(value) do
		if typeof(key) == "string" then
			local encoded = encodeTypedValue(entry)
			if encoded ~= nil then
				result[key] = encoded
			end
		end
	end
	return result
end

local function normalizeTags(tags)
	local result = {}
	if typeof(tags) ~= "table" then
		return result
	end

	for _, tag in ipairs(tags) do
		if typeof(tag) == "string" then
			table.insert(result, tag)
		end
	end

	return result
end

local function isInternalManagedAttribute(name)
	return name == MANAGED_ATTRIBUTE or name == MANAGED_CHILD_KEYS_ATTRIBUTE
end

local function sanitizeSnapshotAttributes(attributes)
	local result = {}
	if typeof(attributes) ~= "table" then
		return result
	end

	for name, value in pairs(attributes) do
		if typeof(name) == "string" and not isInternalManagedAttribute(name) then
			local encoded = encodeTypedValue(value)
			if encoded ~= nil then
				result[name] = encoded
			end
		end
	end

	return result
end

local function snapshotStudioNode(instance)
	local properties = extractStudioProperties(instance)
	if properties.FontFace ~= nil then
		-- Font (Enum) is the legacy view of FontFace and reads as Unknown for
		-- custom fonts; FontFace is authoritative.
		properties.Font = nil
	end
	local node = {
		name = instance.Name,
		className = instance.ClassName,
		children = {},
		properties = normalizeDictionary(properties),
		attributes = sanitizeSnapshotAttributes(instance:GetAttributes()),
		tags = normalizeTags(CollectionService:GetTags(instance)),
	}

	if instance:IsA("LuaSourceContainer") then
		local ok, source = pcall(function()
			return instance.Source
		end)
		if ok and typeof(source) == "string" then
			node.source = source
		end
	end

	for _, child in ipairs(instance:GetChildren()) do
		local isTrash = instance == ServerStorage and child.Name == TRASH_FOLDER_NAME
		if not isTrash then
			table.insert(node.children, snapshotStudioNode(child))
		end
	end

	return node
end

local function captureStudioTree()
	local rootNode = {
		name = "Studio",
		className = "DataModel",
		children = {},
		properties = emptyObject(),
		attributes = emptyObject(),
		tags = {},
	}

	for serviceName, _ in pairs(ROOT_SERVICE_CLASSES) do
		local ok, service = pcall(function()
			return game:GetService(serviceName)
		end)
		if ok and service ~= nil then
			table.insert(rootNode.children, snapshotStudioNode(service))
		end
	end

	table.sort(rootNode.children, function(left, right)
		return left.name < right.name
	end)

	return rootNode
end

local function nodeKey(name, className)
	return tostring(className) .. "\0" .. tostring(name)
end

local function isManaged(instance)
	return instance:GetAttribute(MANAGED_ATTRIBUTE) == MANAGED_VALUE
end

local function summarizeCounters(counters)
	return string.format(
		"created=%d\nupdated=%d\ndeleted (moved to trash)=%d\nleft in Studio (deletes off)=%d\nskipped=%d",
		counters.created,
		counters.updated,
		counters.deleted,
		counters.keptDeletes or 0,
		counters.skipped
	)
end

local function safeSetProperty(instance, propertyName, value)
	if typeof(propertyName) ~= "string" or PROTECTED_PROPERTIES[propertyName] then
		return false
	end

	local decoded = decodeTypedValue(value)
	if decoded == nil then
		return false
	end

	local ok = pcall(function()
		instance[propertyName] = decoded
	end)
	return ok
end

local function applyPropertyMap(instance, propertyMap, propertyNames)
	for _, propertyName in ipairs(propertyNames) do
		local value = propertyMap[propertyName]
		if value ~= nil then
			safeSetProperty(instance, propertyName, value)
		end
	end
end

local function applyAttributes(instance, desiredAttributes)
	local desired = {}
	if typeof(desiredAttributes) == "table" then
		for name, value in pairs(desiredAttributes) do
			local decoded = decodeTypedValue(value)
			if decoded ~= nil then
				desired[name] = true
				local ok = pcall(function()
					instance:SetAttribute(name, decoded)
				end)
				if not ok then
					warn(string.format("Nyjo: skipped protected attribute %s on %s", tostring(name), instance:GetFullName()))
				end
			end
		end
	end

	if isManaged(instance) then
		for name, _ in pairs(instance:GetAttributes()) do
			if not isInternalManagedAttribute(name) and not desired[name] then
				local ok = pcall(function()
					instance:SetAttribute(name, nil)
				end)
				if not ok then
					warn(string.format("Nyjo: could not clear protected attribute %s on %s", tostring(name), instance:GetFullName()))
				end
			end
		end
	end
end

local function applyTags(instance, desiredTags)
	if typeof(desiredTags) ~= "table" then
		return
	end

	local desired = {}
	for _, tag in ipairs(desiredTags) do
		if typeof(tag) == "string" then
			desired[tag] = true
			if not CollectionService:HasTag(instance, tag) then
				CollectionService:AddTag(instance, tag)
			end
		end
	end

	if isManaged(instance) then
		for _, tag in ipairs(CollectionService:GetTags(instance)) do
			if not desired[tag] then
				CollectionService:RemoveTag(instance, tag)
			end
		end
	end
end

local function applyProperties(instance, node)
	if typeof(node.properties) == "table" then
		if instance:IsA("UICorner") then
			applyPropertyMap(instance, node.properties, UI_CORNER_SHORTHAND_PROPERTIES)
			applyPropertyMap(instance, node.properties, UI_CORNER_INDIVIDUAL_PROPERTIES)

			for propertyName, value in pairs(node.properties) do
				if propertyName ~= "CornerRadius" and not UI_CORNER_INDIVIDUAL_PROPERTY_SET[propertyName] then
					safeSetProperty(instance, propertyName, value)
				end
			end
		else
			local hasFontFace = node.properties.FontFace ~= nil
			for propertyName, value in pairs(node.properties) do
				if not (hasFontFace and propertyName == "Font") then
					safeSetProperty(instance, propertyName, value)
				end
			end
		end
	end

	applyAttributes(instance, node.attributes)
	applyTags(instance, node.tags)
end

local function markManaged(instance)
	instance:SetAttribute(MANAGED_ATTRIBUTE, MANAGED_VALUE)
end

local function readManagedChildKeys(parent)
	local result = {}
	local raw = parent:GetAttribute(MANAGED_CHILD_KEYS_ATTRIBUTE)
	if typeof(raw) ~= "string" or raw == "" then
		return result
	end

	local ok, decoded = pcall(function()
		return HttpService:JSONDecode(raw)
	end)
	if not ok or typeof(decoded) ~= "table" then
		return result
	end

	for _, key in ipairs(decoded) do
		if typeof(key) == "string" then
			result[key] = true
		end
	end

	return result
end

local function saveStudioBackupMarker(backup)
	if typeof(backup) ~= "table" or typeof(backup.id) ~= "string" or backup.id == "" then
		return
	end

	plugin:SetSetting(SETTINGS_KEY_LAST_STUDIO_BACKUP_ID, backup.id)

	local placeId = backup.placeId
	if typeof(placeId) == "number" and placeId > 0 then
		plugin:SetSetting(SETTINGS_KEY_LAST_STUDIO_BACKUP_PLACE_ID, placeId)
	else
		plugin:SetSetting(SETTINGS_KEY_LAST_STUDIO_BACKUP_PLACE_ID, nil)
	end

	local label = backup.label
	if typeof(label) == "string" and label ~= "" then
		plugin:SetSetting(SETTINGS_KEY_LAST_STUDIO_BACKUP_LABEL, label)
	else
		plugin:SetSetting(SETTINGS_KEY_LAST_STUDIO_BACKUP_LABEL, backup.id)
	end
end

local function readStudioBackupMarker()
	local backupId = plugin:GetSetting(SETTINGS_KEY_LAST_STUDIO_BACKUP_ID)
	if typeof(backupId) ~= "string" or backupId == "" then
		return nil
	end

	local placeId = plugin:GetSetting(SETTINGS_KEY_LAST_STUDIO_BACKUP_PLACE_ID)
	local label = plugin:GetSetting(SETTINGS_KEY_LAST_STUDIO_BACKUP_LABEL)
	return {
		id = backupId,
		placeId = typeof(placeId) == "number" and placeId or nil,
		label = typeof(label) == "string" and label ~= "" and label or backupId,
	}
end

local function resolveService(node)
	if ROOT_SERVICE_CLASSES[node.className] ~= true then
		return nil, "top-level node " .. tostring(node.name) .. " is not a supported service class"
	end

	local ok, service = pcall(function()
		return game:GetService(node.className)
	end)

	if not ok or service == nil then
		return nil, "failed to resolve service " .. tostring(node.className)
	end

	return service
end

-- Defined further down (needs the bridge state); declared here because the
-- backup helpers call it. Calling it before this forward declaration existed
-- made every push fail with "attempt to call a nil value".
local resolveBridgePlaceName

local function writeManagedChildKeys(parent, keySet)
	local keys = {}
	for key, _ in pairs(keySet) do
		table.insert(keys, key)
	end
	table.sort(keys)

	local ok, encoded = pcall(function()
		return HttpService:JSONEncode(keys)
	end)
	if not ok then
		return
	end

	pcall(function()
		parent:SetAttribute(MANAGED_CHILD_KEYS_ATTRIBUTE, encoded)
	end)
end

local function describePath(parentPath, name)
	if parentPath == nil or parentPath == "" then
		return tostring(name)
	end
	return parentPath .. "." .. tostring(name)
end

-- Validates one node coming from the server (local files or a backup) and
-- returns an error string for anything Nyjo must not try to apply.
local function invalidNodeReason(node)
	if typeof(node) ~= "table" then
		return "node is not an object"
	end
	if typeof(node.name) ~= "string" or node.name == "" then
		return "node has no name"
	end
	if #node.name > 100 then
		return "name is longer than Roblox allows (100 characters)"
	end
	if typeof(node.className) ~= "string" or node.className == "" then
		return "node has no className"
	end
	if node.children ~= nil and typeof(node.children) ~= "table" then
		return "children is not a list"
	end
	return nil
end

local function countTreeNodes(node)
	local total = 1
	if typeof(node) == "table" and typeof(node.children) == "table" then
		for _, child in ipairs(node.children) do
			total = total + countTreeNodes(child)
		end
	end
	return total
end

-- Returns desiredByKey (first node per key wins), the ordered key list, and
-- pushes a warning for duplicate or invalid nodes.
local function buildDesiredMap(children, parentPath, warnings)
	local desiredByKey = {}
	local orderedKeys = {}

	if typeof(children) ~= "table" then
		return desiredByKey, orderedKeys
	end

	for _, node in ipairs(children) do
		local reason = invalidNodeReason(node)
		if reason then
			table.insert(warnings, string.format("skipped invalid node under %s: %s", tostring(parentPath), reason))
		else
			local key = nodeKey(node.name, node.className)
			if desiredByKey[key] then
				table.insert(
					warnings,
					string.format(
						"%s has more than one local %s named %q; only the first one is synced (use unique names)",
						tostring(parentPath),
						node.className,
						node.name
					)
				)
			else
				desiredByKey[key] = node
				table.insert(orderedKeys, key)
			end
		end
	end

	return desiredByKey, orderedKeys
end

local function findMatchingChildren(parent, node)
	local matches = {}
	for _, child in ipairs(parent:GetChildren()) do
		if child.Name == node.name and child.ClassName == node.className then
			table.insert(matches, child)
		end
	end
	return matches
end

-- Phase 1: work out what a push would do without touching Studio.
local function planChildren(parent, desiredChildren, parentPath, plan)
	local desiredByKey, orderedKeys = buildDesiredMap(desiredChildren, parentPath, plan.warnings)
	local previousManagedKeys = readManagedChildKeys(parent)

	for _, key in ipairs(orderedKeys) do
		local node = desiredByKey[key]
		local path = describePath(parentPath, node.name)
		local matches = findMatchingChildren(parent, node)
		if #matches > 1 then
			table.insert(
				plan.warnings,
				string.format(
					"%s: %d Studio instances share this name and class; only the first one is updated",
					path,
					#matches
				)
			)
		end

		if matches[1] then
			plan.updates = plan.updates + 1
			planChildren(matches[1], node.children, path, plan)
		else
			local unsafeReason = UNSAFE_CREATE_CLASS_REASONS[node.className]
			if unsafeReason then
				plan.skipped = plan.skipped + 1
				table.insert(plan.warnings, string.format("%s: will not create %s (%s)", path, node.className, unsafeReason))
			else
				plan.creates = plan.creates + countTreeNodes(node)
			end
		end
	end

	-- Deletion candidates: only instances Nyjo itself synced into this parent
	-- on an earlier push (marked managed AND listed in the parent's managed
	-- key list) whose key is gone from the local tree.
	for _, child in ipairs(parent:GetChildren()) do
		if isManaged(child) then
			local key = nodeKey(child.Name, child.ClassName)
			if previousManagedKeys[key] and desiredByKey[key] == nil then
				table.insert(plan.deletes, {
					instance = child,
					path = describePath(parentPath, child.Name),
					descendants = #child:GetDescendants(),
				})
				plan.deletedInstances = plan.deletedInstances + 1 + #child:GetDescendants()
			end
		end
	end
end

local function planPush(tree)
	local plan = {
		creates = 0,
		updates = 0,
		skipped = 0,
		deletes = {},
		deletedInstances = 0,
		warnings = {},
		services = {},
	}

	for _, serviceNode in ipairs(tree.children or {}) do
		local reason = invalidNodeReason(serviceNode)
		if reason then
			plan.skipped = plan.skipped + 1
			table.insert(plan.warnings, "skipped invalid top-level node: " .. reason)
		else
			local service, errorMessage = resolveService(serviceNode)
			if service then
				table.insert(plan.services, { service = service, node = serviceNode })
				planChildren(service, serviceNode.children, serviceNode.name, plan)
			else
				plan.skipped = plan.skipped + 1
				table.insert(plan.warnings, errorMessage)
			end
		end
	end

	return plan
end

local function getTrashFolder(batchLabel)
	local trash = ServerStorage:FindFirstChild(TRASH_FOLDER_NAME)
	if trash == nil or not trash:IsA("Folder") then
		trash = Instance.new("Folder")
		trash.Name = TRASH_FOLDER_NAME
		trash:SetAttribute("NyjoNote", "Instances removed by Nyjo pushes. Safe to delete once you are sure you do not need them.")
		trash.Parent = ServerStorage
	end

	local batch = trash:FindFirstChild(batchLabel)
	if batch == nil then
		batch = Instance.new("Folder")
		batch.Name = batchLabel
		batch.Parent = trash
	end
	return batch
end

-- Soft delete: move into ServerStorage.NyjoTrash/<batch> (scripts stop
-- running there) instead of Destroy(), so nothing is ever unrecoverable.
local function trashInstance(instance, originalPath, batchLabel)
	local batch = getTrashFolder(batchLabel)
	instance:SetAttribute("NyjoOriginalPath", originalPath)
	instance:SetAttribute(MANAGED_ATTRIBUTE, nil)
	instance.Parent = batch
end

local function setScriptSource(instance, source, path, warnings)
	local okRead, current = pcall(function()
		return instance.Source
	end)
	if okRead and current == source then
		return
	end

	local ok, setError = pcall(function()
		instance.Source = source
	end)
	if ok then
		return
	end

	-- Collaborative editing / open drafts can reject direct Source writes;
	-- ScriptEditorService is the supported path for those.
	local okAsync, asyncError = pcall(function()
		ScriptEditorService:UpdateSourceAsync(instance, function()
			return source
		end)
	end)
	if not okAsync then
		table.insert(
			warnings,
			string.format("%s: could not update script source (%s / %s)", path, tostring(setError), tostring(asyncError))
		)
	end
end

local function applyNode(instance, node, path, context)
	applyProperties(instance, node)
	if typeof(node.source) == "string" and instance:IsA("LuaSourceContainer") then
		setScriptSource(instance, node.source, path, context.warnings)
	end
end

local applyChildren

local function createInstanceTree(parent, node, path, context)
	local unsafeReason = UNSAFE_CREATE_CLASS_REASONS[node.className]
	if unsafeReason then
		context.counters.skipped = context.counters.skipped + 1
		return
	end

	local instance
	local ok, createError = pcall(function()
		instance = Instance.new(node.className)
	end)
	if not ok or instance == nil then
		context.counters.skipped = context.counters.skipped + 1
		table.insert(context.warnings, string.format("%s: cannot create class %s (%s)", path, tostring(node.className), tostring(createError)))
		return
	end

	instance.Name = node.name
	markManaged(instance)
	-- Configure before parenting so scripts never run with stale contents.
	applyNode(instance, node, path, context)
	applyChildren(instance, node.children, path, context)
	instance.Parent = parent
	context.counters.created = context.counters.created + 1
end

applyChildren = function(parent, desiredChildren, parentPath, context)
	local desiredByKey, orderedKeys = buildDesiredMap(desiredChildren, parentPath, {})
	local previousManagedKeys = readManagedChildKeys(parent)
	local managedKeysAfter = {}

	for _, key in ipairs(orderedKeys) do
		local node = desiredByKey[key]
		local path = describePath(parentPath, node.name)
		local existing = findMatchingChildren(parent, node)[1]
		if existing then
			if not isManaged(existing) then
				-- Adopt exact name/class matches; unrelated Studio content
				-- stays unmanaged and is never deleted by Nyjo.
				markManaged(existing)
			end
			applyNode(existing, node, path, context)
			applyChildren(existing, node.children, path, context)
			context.counters.updated = context.counters.updated + 1
			managedKeysAfter[key] = true
		elseif UNSAFE_CREATE_CLASS_REASONS[node.className] == nil then
			createInstanceTree(parent, node, path, context)
			managedKeysAfter[key] = true
		else
			context.counters.skipped = context.counters.skipped + 1
		end
	end

	for _, child in ipairs(parent:GetChildren()) do
		if isManaged(child) then
			local key = nodeKey(child.Name, child.ClassName)
			if previousManagedKeys[key] and desiredByKey[key] == nil then
				if context.allowDeletes then
					trashInstance(child, describePath(parentPath, child.Name), context.trashBatch)
					context.counters.deleted = context.counters.deleted + 1
				else
					-- Keep tracking it so a later --allow-deletes push can
					-- still clean it up.
					managedKeysAfter[key] = true
					context.counters.keptDeletes = context.counters.keptDeletes + 1
				end
			end
		end
	end

	writeManagedChildKeys(parent, managedKeysAfter)
end

local function buildPushMessage(headline, counters, warnings, notes)
	local message = headline .. "\n\n" .. summarizeCounters(counters)

	local noteLines = {}
	if typeof(notes) == "table" then
		for _, note in ipairs(notes) do
			if typeof(note) == "string" and note ~= "" then
				table.insert(noteLines, note)
			end
		end
	end
	if #noteLines > 0 then
		message = message .. "\n\n" .. table.concat(noteLines, "\n")
	end

	if #warnings > 0 then
		local shown = {}
		for index = 1, math.min(#warnings, 25) do
			table.insert(shown, warnings[index])
		end
		message = message .. "\n\nWarnings:\n- " .. table.concat(shown, "\n- ")
		if #warnings > #shown then
			message = message .. string.format("\n- ... and %d more", #warnings - #shown)
		end
	end

	return message
end

local function formatDeleteList(deletes, limit)
	local lines = {}
	for index, entry in ipairs(deletes) do
		if index > limit then
			table.insert(lines, string.format("... and %d more", #deletes - limit))
			break
		end
		if entry.descendants > 0 then
			table.insert(lines, string.format("%s (+%d descendants)", entry.path, entry.descendants))
		else
			table.insert(lines, entry.path)
		end
	end
	return table.concat(lines, "\n")
end

local function pushTree(tree, updateStatus, options)
	if typeof(tree) ~= "table" or typeof(tree.children) ~= "table" then
		error("server response did not contain a tree")
	end
	options = typeof(options) == "table" and options or {}

	if RunService:IsRunning() then
		error("Studio is in a playtest; stop the playtest before pushing (changes made during play are discarded)")
	end

	local allowDeletes = options.allowDeletes == true
	local plan = planPush(tree)

	if allowDeletes and plan.deletedInstances > MAX_PUSH_DELETED_INSTANCES then
		error(string.format(
			"Push refused: it would remove %d instances (limit %d). Nothing was changed.\n\nWould remove:\n%s\n\nIf this is intended, delete them in Studio by hand, or push in smaller steps.",
			plan.deletedInstances,
			MAX_PUSH_DELETED_INSTANCES,
			formatDeleteList(plan.deletes, 20)
		))
	end

	local counters = {
		created = 0,
		updated = 0,
		deleted = 0,
		keptDeletes = 0,
		skipped = plan.skipped,
	}
	local warnings = plan.warnings
	local context = {
		counters = counters,
		warnings = warnings,
		allowDeletes = allowDeletes,
		trashBatch = DateTime.now():FormatUniversalTime("YYYYMMDD[T]HHmmss[Z]", "en-us") .. "-" .. string.sub(HttpService:GenerateGUID(false), 1, 8),
	}

	local recording = ChangeHistoryService:TryBeginRecording("Nyjo Push")
	if recording == nil then
		error("Studio refused to start an undo recording (another plugin may be recording); nothing was changed")
	end

	local ok, applyError = xpcall(function()
		for _, entry in ipairs(plan.services) do
			applyChildren(entry.service, entry.node.children, entry.node.name, context)
		end
	end, debug.traceback)

	if not ok then
		-- Cancel reverts everything captured since TryBeginRecording.
		ChangeHistoryService:FinishRecording(recording, Enum.FinishRecordingOperation.Cancel)
		error("Push failed and was rolled back; Studio is unchanged.\n\n" .. tostring(applyError))
	end
	ChangeHistoryService:FinishRecording(recording, Enum.FinishRecordingOperation.Commit)

	local notes = {}
	if counters.keptDeletes > 0 then
		table.insert(
			notes,
			string.format(
				"%d instance(s) no longer exist locally but were LEFT in Studio (deletes are off by default):\n%s\nPush with allowDeletes (CLI: nyjo push --allow-deletes) to move them to ServerStorage.%s.",
				counters.keptDeletes,
				formatDeleteList(plan.deletes, 10),
				TRASH_FOLDER_NAME
			)
		)
	end
	if counters.deleted > 0 then
		table.insert(notes, string.format("Removed instances were moved to ServerStorage.%s.%s", TRASH_FOLDER_NAME, context.trashBatch))
	end
	table.insert(notes, "Undo with Ctrl+Z (one step: \"Nyjo Push\").")

	local backup = options.backup
	if typeof(backup) == "table" then
		table.insert(notes, "Studio backup saved: " .. tostring(backup.label or backup.id or "unknown"))
	end

	local restoredBackup = options.restoredBackup
	if typeof(restoredBackup) == "table" then
		table.insert(notes, "Restored Studio from: " .. tostring(restoredBackup.label or restoredBackup.id or "unknown"))
	end

	local rescueBackup = options.rescueBackup
	if typeof(rescueBackup) == "table" then
		table.insert(notes, "Current Studio state rescued as: " .. tostring(rescueBackup.label or rescueBackup.id or "unknown"))
	end

	local headline = "Push complete."
	if typeof(options.headline) == "string" and options.headline ~= "" then
		headline = options.headline
	end

	local message = buildPushMessage(headline, counters, warnings, notes)
	if updateStatus ~= false then
		setStatus(message)
	end
	return {
		message = message,
		counters = counters,
		warnings = warnings,
	}
end

local function createStudioBackup(reason, tree, remember)
	local response = postJson("/api/backups/studio/create", {
		sessionId = BRIDGE_SESSION_ID,
		placeName = resolveBridgePlaceName(),
		placeId = game.PlaceId ~= 0 and game.PlaceId or nil,
		reason = reason,
		tree = tree,
	})
	local backup = response and response.data and response.data.backup
	if typeof(backup) ~= "table" or typeof(backup.id) ~= "string" or backup.id == "" then
		error("server response did not contain a Studio backup")
	end

	if remember ~= false then
		saveStudioBackupMarker(backup)
	end

	return backup
end

local operationInProgress = nil

-- Only one push/pull/restore at a time, whether it came from a button or
-- from the dashboard/CLI bridge.
local function runExclusive(label, fn, ...)
	if operationInProgress ~= nil then
		error(string.format("Nyjo is busy (%s); try again when it finishes", operationInProgress), 0)
	end
	operationInProgress = label
	local results = table.pack(pcall(fn, ...))
	operationInProgress = nil
	if not results[1] then
		error(results[2], 0)
	end
	return table.unpack(results, 2, results.n)
end

local function ensureCompatibleServer()
	local decoded = fetchJson("/api/health")
	local protocol = decoded and decoded.data and decoded.data.bridgeProtocol
	if protocol ~= BRIDGE_VERSION then
		error(string.format(
			"nyjo server speaks %s but this plugin speaks %s. Rebuild nyjo, run `nyjo install`, and restart Studio.",
			tostring(protocol or "an older protocol"),
			BRIDGE_VERSION
		), 0)
	end
end

local function pushLocalTree(updateStatus, allowDeletes)
	ensureCompatibleServer()
	-- Fetch and validate the local tree before touching anything, so a parse
	-- error on disk can never turn into a destructive push.
	local decoded = fetchJson("/api/tree")
	local tree = decoded and decoded.data and decoded.data.tree
	if typeof(tree) ~= "table" or typeof(tree.children) ~= "table" then
		error("server response did not contain a tree; nothing was changed", 0)
	end
	local backup = createStudioBackup("before push local tree", captureStudioTree(), true)
	return pushTree(tree, updateStatus, {
		backup = backup,
		allowDeletes = allowDeletes == true,
	})
end

local function restoreStudioBackup(updateStatus)
	ensureCompatibleServer()
	local savedBackup = readStudioBackupMarker()
	if savedBackup == nil then
		error("no saved Studio backup is available yet; push to Studio once first")
	end

	if typeof(savedBackup.placeId) == "number" and savedBackup.placeId > 0 and game.PlaceId ~= 0 and savedBackup.placeId ~= game.PlaceId then
		error("the saved Studio backup belongs to a different place; open the matching place or create a fresh backup here first")
	end

	local rescueBackup = createStudioBackup("before restoring studio backup", captureStudioTree(), false)
	local response = postJson("/api/backups/studio/read", {
		backupId = savedBackup.id,
	})
	local data = response and response.data or {}
	local tree = data.tree
	if typeof(tree) ~= "table" then
		error("backup response did not contain a tree")
	end

	return pushTree(tree, updateStatus, {
		headline = "Studio restore complete.",
		restoredBackup = data.backup,
		rescueBackup = rescueBackup,
	})
end

local function syncFromStudio(mode, force, updateStatus)
	ensureCompatibleServer()
	if RunService:IsRunning() then
		error("Studio is in a playtest; stop it before pulling so play-mode state is not written to disk", 0)
	end
	local tree = captureStudioTree()
	local response = postJson("/api/sync-from-studio", {
		tree = tree,
		mode = mode,
		force = force,
	})

	local data = response and response.data or {}
	local plan = data.plan or {}
	local stats = plan.stats or {}
	local returnedTree = data.tree
	local studioNodes = 0
	for _, serviceNode in ipairs(tree.children) do
		studioNodes = studioNodes + countSnapshotDescendants(serviceNode)
	end

	local headline
	if mode == "preview" then
		headline = "Preview Pull ready."
	elseif data.blocked then
		headline = "Apply Pull blocked."
	elseif force then
		headline = "Force Pull complete."
	else
		headline = "Apply Pull complete."
	end

	local message = (
		"%s\n\nStudio nodes captured: %d\nMode: %s\nApplied: %s\nBlocked: %s\nSupported services: %s\nPlanned directories/files/metadata/removals: %s/%s/%s/%s\nWritten directories/files/metadata/removals: %s/%s/%s/%s\nConflicts: %s\nSkipped: %s\nUnchanged: %s"
	):format(
		headline,
		studioNodes,
		tostring(data.mode or mode),
		tostring(data.applied or false),
		tostring(data.blocked or false),
		tostring(stats.supportedServices or 0),
		tostring(stats.directoriesPlanned or 0),
		tostring(stats.filesPlanned or 0),
		tostring(stats.metadataPlanned or 0),
		tostring(stats.removalsPlanned or 0),
		tostring(stats.directoriesWritten or 0),
		tostring(stats.filesWritten or 0),
		tostring(stats.metadataWritten or 0),
		tostring(stats.removalsApplied or 0),
		tostring(stats.conflicts or 0),
		tostring(stats.skippedNodes or 0),
		tostring(stats.unchanged or 0)
	)

	if typeof(returnedTree) == "table" then
		local rootName = returnedTree.name or "unknown"
		local nodeCount = countNodes(returnedTree)
		local preview = buildTreePreview(returnedTree, 16)
		message = message .. ("\nLocal root: %s\nLocal nodes: %d\n\n%s"):format(tostring(rootName), nodeCount, preview)
	end

	message = message .. "\n\nChanged paths:\n" .. formatOperationLines(plan.operations, 18)

	if typeof(plan.conflicts) == "table" and #plan.conflicts > 0 then
		message = message .. "\n\nConflicts:\n" .. formatConflictLines(plan.conflicts, 8)
	end

	local backup = data.backup
	if typeof(backup) == "table" then
		message = message .. "\n\nLocal backup saved: " .. tostring(backup.label or backup.id or "unknown")
		if typeof(backup.restoreHint) == "string" and backup.restoreHint ~= "" then
			message = message .. "\nRestore: " .. backup.restoreHint
		end
	end

	if data.blocked then
		message = message .. "\n\nUse Force Pull after reviewing the conflicts if you want Studio to win."
	end

	if updateStatus ~= false then
		setStatus(message)
	end
	return {
		message = message,
		response = data,
		studioNodes = studioNodes,
	}
end

local bridgeState = {
	status = "idle",
	lastError = nil,
}

local placeInfoCache = {
	placeId = nil,
	resolvedName = nil,
	lastAttempt = 0,
}

resolveBridgePlaceName = function()
	local placeId = game.PlaceId
	local fallbackName = game.Name
	if placeId == 0 then
		return fallbackName
	end

	if placeInfoCache.placeId ~= placeId then
		placeInfoCache.placeId = placeId
		placeInfoCache.resolvedName = nil
		placeInfoCache.lastAttempt = 0
	end

	if typeof(placeInfoCache.resolvedName) == "string" and placeInfoCache.resolvedName ~= "" then
		return placeInfoCache.resolvedName
	end

	local now = os.clock()
	if now - placeInfoCache.lastAttempt < 30 then
		return fallbackName
	end
	placeInfoCache.lastAttempt = now

	local getProductInfo = MarketplaceService.GetProductInfoAsync or MarketplaceService.GetProductInfo
	if getProductInfo == nil then
		return fallbackName
	end

	local ok, info = pcall(getProductInfo, MarketplaceService, placeId, Enum.InfoType.Asset)
	if ok and typeof(info) == "table" then
		local name = info.Name
		if typeof(name) == "string" and name ~= "" then
			placeInfoCache.resolvedName = name
			return name
		end
	end

	return fallbackName
end

local function reportBridgeHeartbeat()
	local heartbeatStatus = bridgeState.status
	if bridgeState.lastError ~= nil then
		heartbeatStatus = "error: " .. bridgeState.lastError
	end

	return tryPostJson("/api/plugin/heartbeat", {
		sessionId = BRIDGE_SESSION_ID,
		bridgeVersion = BRIDGE_VERSION,
		placeName = resolveBridgePlaceName(),
		placeId = game.PlaceId ~= 0 and game.PlaceId or nil,
		status = heartbeatStatus,
	})
end

local function reportBridgeCommandResult(commandId, ok, summary, detail, data)
	tryPostJson("/api/plugin/command-result", {
		commandId = commandId,
		sessionId = BRIDGE_SESSION_ID,
		ok = ok,
		summary = summary,
		detail = detail,
		data = data,
	})
end

local function executeBridgeCommand(command)
	local kind = command and command.kind
	if kind == "pushLocalTree" then
		local result = runExclusive("push", pushLocalTree, true, command.allowDeletes == true)
		return true, "Push complete", result.message, {
			counters = result.counters,
			warnings = result.warnings,
		}
	end

	if kind == "previewPull" then
		local result = runExclusive("preview pull", syncFromStudio, "preview", false, true)
		return true, "Preview Pull ready", result.message, result.response
	end

	if kind == "applyPull" then
		local result = runExclusive("apply pull", syncFromStudio, "apply", false, true)
		return true, "Apply Pull complete", result.message, result.response
	end

	if kind == "forcePull" then
		local result = runExclusive("force pull", syncFromStudio, "apply", true, true)
		return true, "Force Pull complete", result.message, result.response
	end

	error("unsupported bridge command " .. tostring(kind))
end

local function bridgePollOnce()
	local heartbeatOk, heartbeatResult = reportBridgeHeartbeat()
	if not heartbeatOk then
		bridgeState.lastError = tostring(heartbeatResult)
		return false
	end

	local ok, pollResponse = pcall(function()
		return postJson("/api/plugin/poll", {
			sessionId = BRIDGE_SESSION_ID,
		})
	end)
	if not ok then
		bridgeState.lastError = tostring(pollResponse)
		return false
	end

	bridgeState.lastError = nil
	local command = pollResponse and pollResponse.data and pollResponse.data.command
	if typeof(command) ~= "table" or typeof(command.id) ~= "number" then
		return true
	end

	bridgeState.status = "running " .. tostring(command.kind or "command")
	reportBridgeHeartbeat()

	local success, commandOk, summary, detail, data = pcall(executeBridgeCommand, command)
	bridgeState.status = "idle"

	if not success then
		local errorMessage = tostring(commandOk)
		bridgeState.lastError = errorMessage
		reportBridgeCommandResult(
			command.id,
			false,
			"Studio bridge command failed",
			errorMessage,
			nil
		)
		setStatus("Bridge command failed.\n\n" .. errorMessage)
	else
		bridgeState.lastError = nil
		reportBridgeCommandResult(command.id, commandOk, summary, detail, data)
	end

	reportBridgeHeartbeat()
	return true
end

local function handleRequest(label, path, onSuccess)
	local ok, response = pcall(function()
		return fetchJson(path)
	end)

	if not ok then
		setStatus(label .. " failed.\n\n" .. tostring(response))
		return
	end

	onSuccess(response)
end

toggleButton.Click:Connect(function()
	widget.Enabled = not widget.Enabled
end)

widget:GetPropertyChangedSignal("Enabled"):Connect(function()
	toggleButton:SetActive(widget.Enabled)
end)

portBox.FocusLost:Connect(function()
	savePort()
end)

checkButton.MouseButton1Click:Connect(function()
	handleRequest("Server check", "/api/info", function(decoded)
		local data = decoded and decoded.data or {}
		local capabilities = data.capabilities or {}
		local translatedItems = data.translatedItems or {}
		setStatus(
			("Server reachable.\n\nDashboard: %s\nVersion: %s\nBinding: %s\nProject root: %s\n\nCapabilities:\n- web dashboard: %s\n- studio bridge: %s\n- tree preview: %s\n- studio push: %s\n- pull preview: %s\n- pull apply: %s\n- force pull: %s\n\nCompact local files:\n%s\n\nTyped values:\n%s\n\nCommon translated groups:\n%s"):format(
				dashboardUrl(),
				tostring(data.version or "unknown"),
				tostring(data.binding or "unknown"),
				tostring(data.projectRoot or "unknown"),
				tostring(capabilities.webDashboard or false),
				tostring(capabilities.studioBridge or false),
				tostring(capabilities.treePreview or false),
				tostring(capabilities.studioPush or false),
				tostring(capabilities.studioPullPreview or false),
				tostring(capabilities.studioPullApply or false),
				tostring(capabilities.studioPullForce or false),
				formatStringArray(translatedItems.compactFiles, 12),
				formatStringArray(translatedItems.typedValues, 12),
				formatStringArray(translatedItems.commonProperties, 16)
			)
		)
	end)
end)

treeButton.MouseButton1Click:Connect(function()
	handleRequest("Tree fetch", "/api/tree", function(decoded)
		local tree = decoded and decoded.data and decoded.data.tree
		if typeof(tree) ~= "table" then
			setStatus("Tree fetch succeeded, but the response did not contain a tree.")
			return
		end

		local nodeCount = countNodes(tree)
		local rootName = tree.name or "unknown"
		local preview = buildTreePreview(tree, 18)
		setStatus(("Preview only. Studio was not modified.\n\nRoot: %s\nNodes: %d\n\n%s"):format(tostring(rootName), nodeCount, preview))
	end)
end)

pushButton.MouseButton1Click:Connect(function()
	local ok, result = pcall(function()
		return runExclusive("push", pushLocalTree, true, false)
	end)

	if not ok then
		setStatus("Push failed.\n\n" .. tostring(result))
	end
end)

previewPullButton.MouseButton1Click:Connect(function()
	local ok, result = pcall(function()
		return runExclusive("preview pull", syncFromStudio, "preview", false)
	end)

	if not ok then
		setStatus("Preview Pull failed.\n\n" .. tostring(result))
	end
end)

applyPullButton.MouseButton1Click:Connect(function()
	local ok, result = pcall(function()
		return runExclusive("apply pull", syncFromStudio, "apply", false)
	end)

	if not ok then
		setStatus("Apply Pull failed.\n\n" .. tostring(result))
	end
end)

forcePullButton.MouseButton1Click:Connect(function()
	local ok, result = pcall(function()
		return runExclusive("force pull", syncFromStudio, "apply", true)
	end)

	if not ok then
		setStatus("Force Pull failed.\n\n" .. tostring(result))
	end
end)

restoreStudioBackupButton.MouseButton1Click:Connect(function()
	local ok, result = pcall(function()
		return runExclusive("restore", restoreStudioBackup, true)
	end)

	if not ok then
		setStatus("Restore Studio Backup failed.\n\n" .. tostring(result))
	end
end)

portBox.Text = tostring(readPort())

local pluginUnloading = false
plugin.Unloading:Connect(function()
	pluginUnloading = true
end)

if RunService:IsRunning() then
	-- Plugins also load into playtest DataModels. Edits there are thrown away
	-- when the test stops, so never register them as a bridge target.
	setStatus("Nyjo bridge is disabled during playtests. Stop the playtest to sync.")
else
	task.spawn(function()
		local delaySeconds = POLL_INTERVAL_SECONDS
		local wasConnected = nil
		while not pluginUnloading do
			local ok, connected = pcall(bridgePollOnce)
			connected = ok and connected == true
			if connected then
				delaySeconds = POLL_INTERVAL_SECONDS
			else
				-- Back off while the server is down instead of hammering
				-- HttpService (and the Output window) every second.
				delaySeconds = math.min(delaySeconds * 2, MAX_POLL_BACKOFF_SECONDS)
			end
			if connected ~= wasConnected and operationInProgress == nil then
				if connected then
					setStatus("Connected to nyjo at " .. dashboardUrl())
				elseif wasConnected ~= nil then
					setStatus("Lost connection to nyjo at " .. dashboardUrl() .. "; retrying...\n\n" .. tostring(bridgeState.lastError or ""))
				end
			end
			wasConnected = connected
			task.wait(delaySeconds)
		end
	end)
end
widget.Enabled = false
