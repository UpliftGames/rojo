local strict = require(script.Parent.strict)

local isDevBuild = script.Parent.Parent:FindFirstChild("ROJO_DEV_BUILD") ~= nil

local version = { 0, 0, 0, "-dev"}
if script.Parent:FindFirstChild("wally.toml") then
	local wally = require(script.Parent["wally.toml"])
	local versionStr = { string.match(wally.package.version, "^(%d+)%.(%d+)%.(%d+)(.*)$") }
	version = {
		tonumber(versionStr[1]),
		tonumber(versionStr[2]),
		tonumber(versionStr[3]),
		if versionStr[4] == "" then nil else versionStr[4],
	}
end

return strict("Config", {
	isDevBuild = isDevBuild,
	codename = "Epiphany",
	version = version,
	expectedServerVersionString = "7.2 or newer",
	protocolVersion = 4,
	defaultHost = "localhost",
	defaultPort = "34872",
})
