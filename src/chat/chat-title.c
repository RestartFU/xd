#include "chat-title.h"

#include <string.h>

/* How much of the first message becomes the chat's name. */
#define TITLE_LENGTH 48

char *
xd_chat_title_from_prompt (const char *prompt)
{
  g_autofree char *title = NULL;
  const char *newline;

  g_return_val_if_fail (prompt != NULL, NULL);

  newline = strchr (prompt, '\n');
  title = newline != NULL ? g_strndup (prompt, newline - prompt)
                          : g_strdup (prompt);
  g_strstrip (title);

  if (g_utf8_strlen (title, -1) > TITLE_LENGTH)
    {
      g_autofree char *shortened = g_utf8_substring (title, 0, TITLE_LENGTH);

      g_free (title);
      title = g_strconcat (shortened, "…", NULL);
    }

  if (*title == '\0')
    return NULL;

  return g_steal_pointer (&title);
}
