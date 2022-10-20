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