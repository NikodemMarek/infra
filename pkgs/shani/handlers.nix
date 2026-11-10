{
  lib,
  writeText,
  forward-notify,
  session-message,
  matrix-inbox,
}:
# `event-handlers` file (see ../event-handlers/handlers); absolute paths for the handlers outside its package
writeText "shani-handlers" ''
  events.task.new event-handler-task
  events.session.message ${lib.getExe session-message}
  events.claude.> ${lib.getExe forward-notify}
  events.task.started ${lib.getExe forward-notify}
  events.task.failed ${lib.getExe forward-notify}
  matrix.command.shani ${lib.getExe' matrix-inbox "matrix-inbox-command"}
''
