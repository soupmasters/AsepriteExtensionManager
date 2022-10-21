app.range:clear()
app.range.frames = { 4 ... 14 }
app.activeFrame = 4
--os.execute("sleep " .. tonumber(0.04))
app.command.GotoFirstFrameInTag()