#include "workspace-block.h"

#include <string.h>

#define WORKSPACE_OPEN  "<workspace>"
#define WORKSPACE_CLOSE "</workspace>"

static gboolean
starts_line (const char *text,
             const char *at)
{
  return at == text || at[-1] == '\n';
}

static gboolean
ends_line (const char *at)
{
  return *at == '\0' || *at == '\n' ||
         (*at == '\r' && (at[1] == '\0' || at[1] == '\n'));
}

static char *
block_path (const char *open,
            const char *close)
{
  g_autofree char *path =
    g_strndup (open + strlen (WORKSPACE_OPEN),
               close - (open + strlen (WORKSPACE_OPEN)));

  if (strchr (path, '\n') != NULL || strchr (path, '\r') != NULL)
    return NULL;

  g_strstrip (path);
  if (*path == '\0')
    return NULL;

  return g_steal_pointer (&path);
}

char *
xd_workspace_block_parse (const char  *text,
                          char       **remainder)
{
  g_autoptr (GString) prose = NULL;
  g_autofree char *last_path = NULL;
  const char *cursor;
  const char *open;

  if (remainder != NULL)
    *remainder = NULL;
  if (text == NULL)
    return NULL;

  prose = g_string_new (NULL);
  cursor = text;
  open = strstr (cursor, WORKSPACE_OPEN);

  while (open != NULL)
    {
      const char *close = strstr (open + strlen (WORKSPACE_OPEN),
                                  WORKSPACE_CLOSE);
      const char *after =
        close != NULL ? close + strlen (WORKSPACE_CLOSE) : NULL;
      g_autofree char *path =
        close != NULL && starts_line (text, open) && ends_line (after)
          ? block_path (open, close) : NULL;

      if (path == NULL)
        {
          g_string_append_len (prose, cursor,
                               open + strlen (WORKSPACE_OPEN) - cursor);
          cursor = open + strlen (WORKSPACE_OPEN);
          open = strstr (cursor, WORKSPACE_OPEN);
          continue;
        }

      g_string_append_len (prose, cursor, open - cursor);
      g_free (last_path);
      last_path = g_steal_pointer (&path);
      cursor = after;
      if (*cursor == '\r')
        cursor++;
      if (*cursor == '\n')
        cursor++;
      open = strstr (cursor, WORKSPACE_OPEN);
    }

  if (last_path == NULL)
    return NULL;

  g_string_append (prose, cursor);
  g_strstrip (prose->str);
  prose->len = strlen (prose->str);

  if (remainder != NULL)
    *remainder = g_string_free (g_steal_pointer (&prose), FALSE);

  return g_steal_pointer (&last_path);
}
