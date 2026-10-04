-- paged.sheet Excel oracle: open a staged workbook, force a FULL recalculation,
-- save it as .xlsx under a new name, close. Both paths must live inside
-- Excel's own sandbox container (see drive.sh). Returns "OK" or "ERR n: msg";
-- the caller judges success by the output FILE, never by this string.
on run argv
  set src to item 1 of argv
  set dst to item 2 of argv
  tell application "Microsoft Excel"
    set display alerts to false
    try
      open workbook workbook file name src update links do not update links ¬
        ignore read only recommended true notify false add to mru false
      if (count of workbooks) is 0 then error "no workbook opened" number -5000
      set wb to active workbook
      calculate full
      save workbook as wb filename dst file format Excel XML file format
      try
        close wb saving no
      end try
      return "OK"
    on error errMsg number errNum
      try
        close every workbook saving no
      end try
      return "ERR " & errNum & ": " & errMsg
    end try
  end tell
end run
