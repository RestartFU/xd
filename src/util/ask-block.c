#include "ask-block.h"

#include <string.h>

#define ASK_OPEN  "<ask>"
#define ASK_CLOSE "</ask>"

/* More than this and buttons stop being quicker than typing. */
#define MAX_OPTIONS 6

void
hy_ask_free (HyAsk *self)
{
  if (self == NULL)
    return;

  g_free (self->question);
  g_strfreev (self->options);
  g_free (self);
}

const char *
hy_ask_instructions (void)
{
  return
    "<asking_the_user>\n"
    "You cannot prompt for input: you are running non-interactively, and "
    "anything you ask arrives as text the user must answer in a new message.\n\n"
    "When a decision is genuinely the user's to make and the answers are a "
    "short list, wrap the question so it can be shown as buttons:\n\n"
    "<ask>\n"
    "Which approach should I take?\n"
    "- Fix the parser\n"
    "- Rewrite the parser\n"
    "</ask>\n\n"
    "The question goes on the first line and each option on its own line "
    "starting with \"- \". Give two to six options, each a complete answer "
    "rather than a letter. Put the block at the end of your reply.\n\n"
    "Use it only when different answers lead to materially different work. "
    "Anything you can settle yourself, or find out by looking, is not a "
    "question -- decide it and say what you decided.\n"
    "</asking_the_user>";
}

/* Trims and drops the leading list marker, if any. */
static char *
clean_option (const char *line)
{
  const char *start = line;

  while (*start == ' ' || *start == '\t')
    start++;

  if (g_str_has_prefix (start, "- ") || g_str_has_prefix (start, "* "))
    start += 2;

  {
    char *option = g_strdup (start);

    g_strstrip (option);

    return option;
  }
}

HyAsk *
hy_ask_parse (const char  *text,
              char       **remainder)
{
  g_autoptr (GPtrArray) options = NULL;
  g_auto (GStrv) lines = NULL;
  g_autofree char *body = NULL;
  g_autofree char *question = NULL;
  const char *open;
  const char *close;
  HyAsk *ask;

  if (remainder != NULL)
    *remainder = NULL;

  if (text == NULL)
    return NULL;

  open = strstr (text, ASK_OPEN);
  if (open == NULL)
    return NULL;

  close = strstr (open, ASK_CLOSE);
  if (close == NULL)
    return NULL;   /* still streaming, or the assistant never closed it */

  body = g_strndup (open + strlen (ASK_OPEN),
                    close - (open + strlen (ASK_OPEN)));

  lines = g_strsplit (body, "\n", -1);
  options = g_ptr_array_new_with_free_func (g_free);

  for (gsize i = 0; lines[i] != NULL; i++)
    {
      g_autofree char *trimmed = g_strdup (lines[i]);

      g_strstrip (trimmed);
      if (*trimmed == '\0')
        continue;

      /* Everything before the first option is the question. */
      if (!g_str_has_prefix (trimmed, "- ") && !g_str_has_prefix (trimmed, "* "))
        {
          if (options->len == 0)
            {
              if (question == NULL)
                question = g_steal_pointer (&trimmed);
              else
                question = g_strdup_printf ("%s %s", question, trimmed);
            }
          continue;
        }

      if (options->len < MAX_OPTIONS)
        g_ptr_array_add (options, clean_option (trimmed));
    }

  /* One option is not a choice, and none is just prose in angle brackets. */
  if (options->len < 2)
    return NULL;

  if (remainder != NULL)
    {
      g_autofree char *before = g_strndup (text, open - text);
      g_autofree char *after = g_strdup (close + strlen (ASK_CLOSE));

      g_strstrip (before);
      g_strstrip (after);

      *remainder = *after != '\0' ? g_strdup_printf ("%s\n\n%s", before, after)
                                  : g_steal_pointer (&before);
      g_strstrip (*remainder);
    }

  g_ptr_array_add (options, NULL);

  ask = g_new0 (HyAsk, 1);
  ask->question = question != NULL ? g_steal_pointer (&question)
                                   : g_strdup ("Which one?");
  ask->options = (GStrv) g_ptr_array_free (g_steal_pointer (&options), FALSE);

  return ask;
}
