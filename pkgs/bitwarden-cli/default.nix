# `bw` with its data dir (login state, vault cache) moved out of $HOME.
# With the default `dataDir = null` this is upstream bitwarden-cli unchanged.
{
  jail,
  bitwarden-cli,
  dataDir ? null,
}:
if dataDir == null
then bitwarden-cli
else jail bitwarden-cli {BITWARDENCLI_APPDATA_DIR = dataDir;}
