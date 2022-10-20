local ws -- websocket created later
local dlg = Dialog()

local function receive(t, message)
    if t == WebSocketMessageType.OPEN then
        dlg:modify{id="status", text="Sync ON"}
        print("in soup i like")
        ws:sendText("as+´´oa+ifh+fwhi!")

    elseif t == WebSocketMessageType.CLOSE then
        dlg:modify{id="status", text="No connection"}
    end
end


-- set up a websocket
ws = WebSocket{ url="ws://127.0.0.1:8812/Laputa", onreceive=receive, deflate=false }

-- create an UI
dlg:label{ id="status", text="Connecting..." }
dlg:button{ text="Cancel", onclick=finish}

-- let's go!
ws:connect()
dlg:show{ wait=false }