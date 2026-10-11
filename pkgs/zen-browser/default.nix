{
  jail,
  zen-browser,
  dataDir ? null,
}:
if dataDir == null
then zen-browser
else jail zen-browser {XDG_CONFIG_HOME = dataDir;}
