def one(x): (x // "") | gsub("[\r\n\t]"; " ") | .[0:300];

{
  room: "mail",
  body: ("Mail (" + one(.kind) + ") to " + one(.to)
    + "\nFrom: " + one(.from)
    + "\nSubject: " + one(.subject)
    + (if (.snippet | type) == "string" and .snippet != "" then "\n\n" + .snippet[0:1000] else "" end))
}
