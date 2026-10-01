# Commands talk to a background session, not to the app

`launch` and `attach` start a background pokit process that holds the connection to one app instance until `close`; every other command is a request to it. A command that connected on its own would miss whatever happens between commands — console errors, backend output — and could not keep a snapshot's refs alive into the next command, so the cost of owning a process lifecycle (and cleaning it up when an agent stops midway) is taken on deliberately.
