#include "handover.h"

#include <string.h>

/* Roughly how much earlier conversation to hand to a backend that has no
 * session of its own. Enough to carry the thread, not so much that it crowds
 * out the message the user actually asked. */
#define HANDOVER_LIMIT_BYTES 12000

static gboolean
prompt_starts_with_command (const char *prompt)
{
  const char *at;

  if (prompt == NULL || prompt[0] != '/' || prompt[1] == '\0')
    return FALSE;

  for (at = prompt + 1; *at != '\0' && !g_ascii_isspace (*at); at++)
    {
      if (!g_ascii_isalnum (*at) && *at != '_' && *at != '-' && *at != ':')
        return FALSE;
    }

  return at > prompt + 1;
}

char *
xd_handover_join (const char *handover,
                  const char *prompt)
{
  g_return_val_if_fail (prompt != NULL, NULL);

  if (handover == NULL || *handover == '\0')
    return g_strdup (prompt);

  return prompt_starts_with_command (prompt)
    ? g_strdup_printf ("%s\n\n%s", prompt, handover)
    : g_strdup_printf ("%s\n\n%s", handover, prompt);
}

char *
xd_handover_build (XdStorage  *storage,
                   const char *chat_id,
                   gint64      last_seen)
{
  g_autoptr (GPtrArray) messages = NULL;
  g_autoptr (GString) text = NULL;
  gsize budget = 0;
  guint first;

  g_return_val_if_fail (XD_IS_STORAGE (storage), NULL);
  g_return_val_if_fail (chat_id != NULL, NULL);

  messages = xd_storage_list_messages_since (storage, chat_id, last_seen, NULL);
  if (messages == NULL || messages->len < 2)
    return NULL;

  /* Walk back from the most recent, keeping what fits. */
  for (first = messages->len - 1; first > 0; first--)
    {
      const XdMessage *message = g_ptr_array_index (messages, first - 1);

      budget += strlen (message->content) + 16;
      if (budget > HANDOVER_LIMIT_BYTES)
        break;
    }

  text = g_string_new ("[Part of this conversation happened with a different "
                       "assistant, so you have not seen it. It is reproduced "
                       "below verbatim. Treat it as part of the conversation "
                       "you are already in: continue from it, and do not greet "
                       "the user again or re-introduce yourself.]\n\n");

  for (guint i = first; i + 1 < messages->len; i++)
    {
      const XdMessage *message = g_ptr_array_index (messages, i);
      const char *who;

      if (g_strcmp0 (message->role, "user") == 0)
        who = "User";
      else if (g_strcmp0 (message->role, "assistant") == 0)
        who = "Assistant";
      else
        continue;   /* errors and tool notes are ours, not the conversation */

      g_string_append_printf (text, "%s: %s\n\n", who, message->content);
    }

  g_string_append (text, "[End of earlier conversation. The user's new message "
                         "follows.]");

  return g_string_free (g_steal_pointer (&text), FALSE);
}
