#include "ask-block.h"

#include <string.h>

#define ASK_OPEN  "<ask>"
#define ASK_CLOSE "</ask>"

/* More than this and buttons stop being quicker than typing. */
#define MAX_OPTIONS 6

void
xd_ask_free (XdAsk *self)
{
  if (self == NULL)
    return;

  g_free (self->question);
  g_strfreev (self->options);
  g_free (self);
}

const char *
xd_ask_instructions (void)
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
    "When the answer is specific text rather than a short list, put an input "
    "marker on its own line instead:\n\n"
    "<ask>\n"
    "What branch name should I use?\n"
    "<input>\n"
    "</ask>\n\n"
    "This shows a text field. An <ask> may contain either two to six options, "
    "an <input> marker, or both.\n\n"
    "Use it only when different answers lead to materially different work. "
    "Anything you can settle yourself, or find out by looking, is not a "
    "question -- decide it and say what you decided.\n"
    "</asking_the_user>\n\n"
    "<commit_links>\n"
    "When reporting a Git commit, make the hash text a Markdown link to that "
    "commit's web URL. Example: "
    "[abc1234](https://github.com/owner/repo/commit/abc1234). Use the actual "
    "repository URL and hash. Do not report a bare hash when its web URL can "
    "be determined from the repository remote.\n"
    "</commit_links>\n\n"
    "<issue_links>\n"
    "When mentioning a GitHub issue or pull request, make the visible reference "
    "a Markdown link to its web URL whenever the repository and number can be "
    "determined. Examples: "
    "[#35](https://github.com/owner/repo/issues/35) and "
    "[PR #12](https://github.com/owner/repo/pull/12). Do not leave a "
    "resolvable #number as bare text.\n"
    "</issue_links>";
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

/* Reads the block starting at @open, or NULL if what is there is not one. */
static XdAsk *
parse_at (const char  *text,
          const char  *open,
          char       **remainder)
{
  g_autoptr (GPtrArray) options = NULL;
  g_auto (GStrv) lines = NULL;
  g_autofree char *body = NULL;
  g_autofree char *question = NULL;
  const char *close;
  gboolean accepts_input = FALSE;
  XdAsk *ask;

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

      if (g_strcmp0 (trimmed, "<input>") == 0)
        {
          accepts_input = TRUE;
          continue;
        }

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

  /* One option is not a choice. Input-only questions need no fake options. */
  if (options->len < 2 && !accepts_input)
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

  ask = g_new0 (XdAsk, 1);
  ask->question = question != NULL ? g_steal_pointer (&question)
                                   : g_strdup ("Which one?");
  ask->options = (GStrv) g_ptr_array_free (g_steal_pointer (&options), FALSE);
  ask->accepts_input = accepts_input;

  return ask;
}

/*
 * The block the assistant meant, read from the end backwards.
 *
 * Taking the first "<ask>" in the text loses to a reply that talks about the
 * format before using it -- "the response should have been wrapped in
 * <ask>...</ask>" parses as a block with no options, and the real one further
 * down is never reached. The last block that actually holds options or an
 * input is the question being asked; anything earlier is prose that mentions
 * the tag.
 */
static const char *
find_block (const char  *text,
            XdAsk      **out,
            char       **remainder)
{
  const char *found = NULL;
  XdAsk *ask = NULL;

  for (const char *open = strstr (text, ASK_OPEN);
       open != NULL;
       open = strstr (open + 1, ASK_OPEN))
    {
      XdAsk *candidate = parse_at (text, open, NULL);

      if (candidate != NULL)
        {
          g_clear_pointer (&ask, xd_ask_free);
          ask = candidate;
          found = open;
        }
    }

  if (found != NULL && remainder != NULL)
    {
      g_clear_pointer (&ask, xd_ask_free);
      ask = parse_at (text, found, remainder);
    }

  if (out != NULL)
    *out = ask;
  else
    g_clear_pointer (&ask, xd_ask_free);

  return found;
}

XdAsk *
xd_ask_parse (const char  *text,
              char       **remainder)
{
  XdAsk *ask = NULL;

  if (remainder != NULL)
    *remainder = NULL;

  if (text == NULL)
    return NULL;

  find_block (text, &ask, remainder);

  return ask;
}

/*
 * How much of @text can be shown while it is still arriving.
 *
 * The block itself is held back: it becomes buttons once the turn ends, and
 * must not flash past as raw tags first. Only the block that will actually
 * become buttons is hidden -- prose that happens to mention the tag stays
 * visible, since nothing is going to replace it.
 */
gsize
xd_ask_visible_length (const char *text)
{
  const char *block;
  const char *tail;
  gsize length;

  if (text == NULL)
    return 0;

  block = find_block (text, NULL, NULL);
  if (block != NULL)
    return block - text;

  /* A block that has opened but not closed yet is on its way to becoming
   * one, so it is held back too. */
  tail = NULL;
  for (const char *open = strstr (text, ASK_OPEN);
       open != NULL;
       open = strstr (open + 1, ASK_OPEN))
    {
      if (strstr (open, ASK_CLOSE) == NULL)
        {
          tail = open;
          break;
        }
    }

  if (tail != NULL)
    return tail - text;

  /* No complete tag yet. If the tail could still grow into one, hold it. */
  length = strlen (text);
  for (gsize back = MIN (strlen (ASK_OPEN) - 1, length); back > 0; back--)
    {
      if (strncmp (text + length - back, ASK_OPEN, back) == 0)
        return length - back;
    }

  return length;
}
