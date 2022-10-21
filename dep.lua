local ws = WebSocket{
    onreceive = handleMessage,
    url = "http://127.0.0.1:9000",
    deflate = false
}

app.command.OpenFile{ filename="/Users/martin/Dropbox/[ Big Boy Boxing ]/Artwork/Ingame Replica.ase", sequence="agree" }
app.fgColor = { r=5, g=5, b=255, a=255 }
local dlg = Dialog { title = "Bitfrost Animator Unity Link" }

dlg:slider {
    id = "varName1",
    label = "Opacity: ",
    min = 0,
    max = 100,
    value = 50
}
app.alert{title="Title", text="Text", buttons="OK"}

dlg:color {
    id = "varName2",
    label = "Flash Color: ",
    color = Color(255, 128, 64, 255)
}

dlg:combobox {
    id = "varName3",
    label = "Attachment: ",
    option = "Eyes",
    options = {
        "Eyes",
        "Body",
        "Gloves",
        "GlovesTop",
        "Shadow" }
}

dlg:label{ id="varName3",
           label="Made by Soupmasters",
           text="Test" }

dlg:button {
    id = "cancel",
    text = "Disconnect Link",
    onclick = function()
        app.alert("The given value is")
        print("Goodbye!")
        dlg:close()
    end
}

dlg:show { wait = false }

------

local ws -- websocket created later
local dlg = Dialog()

local function receive(t, message)
    if t == WebSocketMessageType.OPEN then
        ws:sendText("Aseprite Connect")
        dlg:modify{id="status", text="Sync ON"}

    elseif t == WebSocketMessageType.TEXT then
        dlg:modify{id="status", text="Aseprite Text: " .. message}

    elseif t == WebSocketMessageType.CLOSE then
        ws:sendText("Aseprite Disconnect")
        dlg:modify{id="status", text="Connection closed"}
    end
end

local function send()
    ws:sendText("Iam seing some data beepboop")
end
-- clean up and exit
local function finish()
    ws:close()
    dlg:close()
end

-- set up a websocket
ws = WebSocket{ url="ws://127.0.0.1:8814/Laputa", onreceive=receive, deflate=false }
--ws://127.0.0.1:8812/Laputa
-- create an UI
dlg:label{ id="status", text="Connecting..." }
dlg:button{ text="Cancel Connection", onclick=finish}
dlg:combobox {
    id = "varName3",
    label = "Attachment: ",
    option = "Eyes",
    options = {
        "Eyes",
        "Body",
        "Gloves",
        "GlovesTop",
        "Shadow" }
}
dlg:button{ text="SendData", onclick=send}

-- let's go!
ws:connect()
dlg:show{ wait=false }

------------------------


disablePattern = "^-"
eventPattern = "*-"

local dlg = Dialog { title = "Animation List" }
function getAnimationTags()
    local s = app.activeSprite
    local t = {}
    for i,tag in ipairs(s.tags) do
        if not (tag.name:find(disablePattern) ~= nil) and (tag.name:find(eventPattern) ~= nil)  then
            table.insert(t, tag.name) --.. tostring(tag):gsub("Tag:", "")
        end
    end
    return t
end


dlg:combobox {
    id = "varName3",
    label = "Animation: ",
    option = "Eyes",
    options = getAnimationTags()
}
dlg:show { wait = false }

local function updateTagColors()
    local s = app.activeSprite
    for i,tag in ipairs(s.tags) do
        if (tag.name:find(disablePattern) ~= nil) then
            tag.color = Color{ r=255, g=255, b=255, a=125 }
        end
        if (tag.name:find(disablePattern) ~= nil) then
            tag.color = Color{ r=255, g=255, b=255, a=125 }
        end
    end
end


app.events:on('sitechange',
        function()
            updateTagColors()
        end)

updateTagColors()
