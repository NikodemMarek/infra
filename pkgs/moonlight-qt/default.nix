{
  jail,
  moonlight-qt,
  dataDir ? null,
}:
if dataDir == null
then moonlight-qt
else jail moonlight-qt {XDG_CONFIG_HOME = dataDir;}
