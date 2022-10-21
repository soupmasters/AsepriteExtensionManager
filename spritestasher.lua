function file_exists(name)
    local f=io.open(name,"r")
    if f~=nil then io.close(f) return true else return false end
end
local pathPart = app.fs.filePath(fn)
local oldFile = "/Users/martin/Desktop"
local newFile = ""
--os.execute(string.format('copy "%s" "%s"', oldFile, newFile))