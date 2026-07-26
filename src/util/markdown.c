#include "markdown.h"

#include <pango/pango.h>
#include <string.h>

/* Finds @needle after @from, or NULL. Used to decide whether a span is
 * actually closed before opening a tag for it. */
static const char *
find_close (const char *from,
            const char *needle)
{
  return strstr (from, needle);
}

static void
append_escaped (GString    *out,
                const char *text,
                gssize      length)
{
  g_autofree char *escaped = g_markup_escape_text (text, length);

  g_string_append (out, escaped);
}

/*
 * Inline spans, in precedence order: code first (nothing nests inside it),
 * then strong, then emphasis.
 *
 * Only '*' marks emphasis. Underscores are left alone on purpose: they turn up
 * in identifiers like some_function far more often than as italics.
 */
static void
append_inline (GString    *out,
               const char *text)
{
  const char *p = text;

  while (*p != '\0')
    {
      if (*p == '`')
        {
          const char *close = find_close (p + 1, "`");

          if (close != NULL)
            {
              g_string_append (out, "<tt>");
              append_escaped (out, p + 1, close - (p + 1));
              g_string_append (out, "</tt>");
              p = close + 1;
              continue;
            }
        }
      else if (p[0] == '*' && p[1] == '*')
        {
          const char *close = find_close (p + 2, "**");

          if (close != NULL)
            {
              g_autofree char *inner = g_strndup (p + 2, close - (p + 2));

              g_string_append (out, "<b>");
              append_inline (out, inner);
              g_string_append (out, "</b>");
              p = close + 2;
              continue;
            }
        }
      else if (p[0] == '[')
        {
          /* [text](url): a link, rendered as one. Both halves have to close
             on the same line for it to count; anything else is prose that
             happens to start with a bracket. */
          const char *close = find_close (p + 1, "]");

          if (close != NULL && close[1] == '(')
            {
              const char *end = find_close (close + 2, ")");

              if (end != NULL && end > close + 2 &&
                  memchr (p, '\n', end - p) == NULL)
                {
                  g_autofree char *label = g_strndup (p + 1, close - (p + 1));
                  g_autofree char *url = g_strndup (close + 2, end - (close + 2));
                  g_autofree char *href = g_markup_escape_text (url, -1);

                  g_string_append_printf (out, "<a href=\"%s\">", href);
                  append_inline (out, label);
                  g_string_append (out, "</a>");
                  p = end + 1;
                  continue;
                }
            }
        }
      else if (p[0] == '*')
        {
          const char *close = find_close (p + 1, "*");

          if (close != NULL && close > p + 1)
            {
              g_autofree char *inner = g_strndup (p + 1, close - (p + 1));

              g_string_append (out, "<i>");
              append_inline (out, inner);
              g_string_append (out, "</i>");
              p = close + 1;
              continue;
            }
        }

      /* Either an ordinary character or an unclosed marker; show it as-is. */
      append_escaped (out, p, 1);
      p++;
    }
}

static gboolean
is_fence (const char *line)
{
  return g_str_has_prefix (line, "```") || g_str_has_prefix (line, "~~~");
}

static void
append_heading (GString    *out,
                const char *line)
{
  const char *text = line;

  while (*text == '#')
    text++;
  while (*text == ' ')
    text++;

  g_string_append (out, "<b>");
  append_inline (out, text);
  g_string_append (out, "</b>");
}

char *
xd_markdown_to_pango (const char *text)
{
  g_autoptr (GString) out = NULL;
  g_auto (GStrv) lines = NULL;
  gboolean in_fence = FALSE;

  if (text == NULL)
    return g_strdup ("");

  out = g_string_new (NULL);
  lines = g_strsplit (text, "\n", -1);

  for (gsize i = 0; lines[i] != NULL; i++)
    {
      const char *line = lines[i];

      if (is_fence (line))
        {
          /* The tag closes even if the block never does, which is the normal
           * state of affairs while a reply is still streaming. */
          g_string_append (out, in_fence ? "</tt>" : "<tt>");
          in_fence = !in_fence;
          continue;
        }

      if (i > 0)
        g_string_append_c (out, '\n');

      if (in_fence)
        append_escaped (out, line, -1);
      else if (line[0] == '#')
        append_heading (out, line);
      else
        {
          const char *item = line;

          while (*item == ' ')
            item++;

          /* A list marker becomes the dot it stands for; the indentation in
           * front of it survives, so nested lists keep their shape. */
          if ((item[0] == '-' || item[0] == '*') && item[1] == ' ')
            {
              append_escaped (out, line, item - line);
              g_string_append (out, "\xe2\x80\xa2 ");
              append_inline (out, item + 2);
            }
          else
            {
              append_inline (out, line);
            }
        }
    }

  if (in_fence)
    g_string_append (out, "</tt>");

  /* Last line of defence: anything Pango will not accept is shown as plain
   * text rather than as nothing at all. Links are stripped first -- <a> is
   * GtkLabel's extension, and Pango's own parser rejects it, which silently
   * vetoed every message containing one. */
  {
    g_autoptr (GString) check = g_string_new (out->str);
    const char *open;

    while ((open = strstr (check->str, "<a href=\"")) != NULL)
      {
        const char *end = strchr (open, '>');

        if (end == NULL)
          break;
        g_string_erase (check, open - check->str, end - open + 1);
      }

    while ((open = strstr (check->str, "</a>")) != NULL)
      g_string_erase (check, open - check->str, 4);

    if (!pango_parse_markup (check->str, -1, 0, NULL, NULL, NULL, NULL))
      {
        g_debug ("markdown produced invalid markup; falling back to plain text");
        return g_markup_escape_text (text, -1);
      }
  }

  return g_string_free (g_steal_pointer (&out), FALSE);
}
