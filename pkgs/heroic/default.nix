{
  jail,
  heroic,
  dataDir ? null,
}:
if dataDir == null
then heroic
else jail heroic {XDG_CONFIG_HOME = dataDir;}
