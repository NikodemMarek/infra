# `gh` with its config dir (config.yml, hosts.yml with the login) moved out of
# $HOME. With the default `dataDir = null` this is upstream gh unchanged.
{
  jail,
  gh,
  dataDir ? null,
}:
if dataDir == null
then gh
else jail gh {GH_CONFIG_DIR = dataDir;}
