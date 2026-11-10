# `glab` with its config dir (config.yml with the logins) moved out of $HOME.
# With the default `dataDir = null` this is upstream glab unchanged.
{
  jail,
  glab,
  dataDir ? null,
}:
if dataDir == null
then glab
else jail glab {GLAB_CONFIG_DIR = dataDir;}
