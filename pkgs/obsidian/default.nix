{
  jail,
  obsidian,
  dataDir ? null,
}:
if dataDir == null
then obsidian
else jail obsidian {XDG_CONFIG_HOME = dataDir;}
