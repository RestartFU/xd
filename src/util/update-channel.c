#include "update-channel.h"

#include <json-glib/json-glib.h>
#include <string.h>

XdUpdateChannel
xd_update_channel_current (void)
{
  if (g_strcmp0 (XD_CHANNEL, "nightly") == 0)
    return XD_UPDATE_CHANNEL_NIGHTLY;

  return XD_UPDATE_CHANNEL_RELEASE;
}

const char *
xd_update_channel_tag (XdUpdateChannel channel)
{
  switch (channel)
    {
    case XD_UPDATE_CHANNEL_NIGHTLY:
      return "nightly";

    case XD_UPDATE_CHANNEL_RELEASE:
    default:
      return NULL;
    }
}

char *
xd_update_channel_check_url (XdUpdateChannel channel)
{
  const char *tag = xd_update_channel_tag (channel);

  /* Not /releases/latest for the rolling channel: that resolves to the newest
   * full release, and the nightly is a prerelease. */
  if (tag != NULL)
    return g_strdup_printf (
      "https://api.github.com/repos/" XD_REPO "/releases/tags/%s", tag);

  return g_strdup ("https://api.github.com/repos/" XD_REPO "/releases/latest");
}

/*
 * The same one-line install anyone would run.
 *
 * The script is published beside the build it installs, so the two are from
 * the same commit -- and it is the script that knows where things go, which
 * keeps that in one place rather than two.
 */
char *
xd_update_channel_install_command (XdUpdateChannel channel)
{
  const char *tag = xd_update_channel_tag (channel);

  if (tag != NULL)
    return g_strdup_printf (
      "curl -fsSL https://github.com/" XD_REPO
      "/releases/download/%s/install.sh | sh", tag);

  return g_strdup ("curl -fsSL https://github.com/" XD_REPO
                   "/releases/latest/download/install.sh | sh -s -- --release");
}

char *
xd_update_channel_latest_from_reply (XdUpdateChannel  channel,
                                     const char      *body)
{
  g_autoptr (JsonParser) parser = json_parser_new ();
  JsonObject *release;

  if (body == NULL)
    return NULL;

  if (!json_parser_load_from_data (parser, body, -1, NULL) ||
      !JSON_NODE_HOLDS_OBJECT (json_parser_get_root (parser)))
    return NULL;

  release = json_node_get_object (json_parser_get_root (parser));

  /* A rolling release keeps its name and changes its commit, so the commit is
   * what identifies it. A release is a tag, so the tag is. */
  return g_strdup (json_object_get_string_member_with_default (
    release,
    xd_update_channel_tag (channel) != NULL ? "target_commitish" : "tag_name",
    NULL));
}

gboolean
xd_update_channel_is_newer (XdUpdateChannel  channel,
                            const char      *latest)
{
  if (latest == NULL || *latest == '\0')
    return FALSE;

  if (xd_update_channel_tag (channel) != NULL)
    {
      /* The API gives the whole commit; this build knows its own short one. */
      if (XD_COMMIT[0] == '\0')
        return FALSE;

      return !g_str_has_prefix (latest, XD_COMMIT);
    }

  /* Tags are written v1.2.3; the version is not. Compared against the bare
   * version rather than the string shown to people, which carries the commit
   * as well. */
  if (latest[0] == 'v')
    latest++;

  return g_strcmp0 (latest, XD_VERSION) != 0;
}
