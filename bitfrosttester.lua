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
dlg:button{ text="Cancel", onclick=finish}
dlg:button{ text="SendData", onclick=send}

-- let's go!
ws:connect()
dlg:show{ wait=false }