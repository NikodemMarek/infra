{
  jail,
  prismlauncher,
  dataDir ? null,
}:
if dataDir == null
then prismlauncher
else jail prismlauncher {XDG_DATA_HOME = dataDir;}
