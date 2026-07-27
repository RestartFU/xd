#include "subagent-tool.h"

#include <string.h>

#define SUBAGENT_PREFIX "subagent\n"
#define IDENTITY_LIMIT 80
#define TASK_LIMIT 320

static char *
one_line (const char *text,
          glong       limit,
          const char *fallback)
{
  g_autoptr (GString) result = g_string_new (NULL);
  gboolean spaced = FALSE;

  if (text == NULL || *text == '\0')
    return g_strdup (fallback);

  for (const char *at = text; *at != '\0'; at = g_utf8_next_char (at))
    {
      gunichar character = g_utf8_get_char (at);

      if (g_unichar_isspace (character))
        {
          spaced = result->len > 0;
          continue;
        }

      if (spaced)
        {
          g_string_append_c (result, ' ');
          spaced = FALSE;
        }

      g_string_append_unichar (result, character);
      if (g_utf8_strlen (result->str, -1) >= limit)
        {
          g_string_append (result, "…");
          break;
        }
    }

  return result->len > 0
    ? g_string_free (g_steal_pointer (&result), FALSE)
    : g_strdup (fallback);
}

char *
xd_subagent_tool_new (const char *identity,
                      const char *task)
{
  g_autofree char *safe_identity =
    one_line (identity, IDENTITY_LIMIT, "General");
  g_autofree char *safe_task =
    one_line (task, TASK_LIMIT, "Delegated task");

  return g_strdup_printf (SUBAGENT_PREFIX "%s\n%s",
                          safe_identity, safe_task);
}

gboolean
xd_subagent_tool_from_tool (const char *message,
                            char      **identity,
                            char      **task)
{
  const char *name;
  const char *newline;
  const char *detail;

  if (message == NULL || !g_str_has_prefix (message, SUBAGENT_PREFIX))
    return FALSE;

  name = message + strlen (SUBAGENT_PREFIX);
  newline = strchr (name, '\n');
  if (newline == NULL)
    return FALSE;

  detail = newline + 1;
  if (newline == name || *detail == '\0' || strchr (detail, '\n') != NULL)
    return FALSE;

  if (identity != NULL)
    *identity = g_strndup (name, newline - name);
  if (task != NULL)
    *task = g_strdup (detail);

  return TRUE;
}
