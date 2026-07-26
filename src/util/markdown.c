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

static gboolean
starts_url (const char *text)
{
  return g_str_has_prefix (text, "https://") ||
         g_str_has_prefix (text, "http://");
}

static guint
count_byte (const char *text,
            gsize       length,
            char        byte)
{
  guint count = 0;

  for (gsize i = 0; i < length; i++)
    count += text[i] == byte;

  return count;
}

/*
 * Stops before prose punctuation while preserving punctuation that belongs to
 * the URL. A closing parenthesis stays when the URL opened one, but the one
 * around "(https://example.com)" does not.
 */
static gsize
url_length (const char *text)
{
  gsize length = 0;

  while (text[length] != '\0' &&
         !g_ascii_isspace (text[length]) &&
         strchr ("<>\"'", text[length]) == NULL)
    length++;

  while (length > 0)
    {
      char last = text[length - 1];

      if (strchr (".,;:!?", last) != NULL)
        {
          length--;
          continue;
        }

      if ((last == ')' &&
           count_byte (text, length, ')') > count_byte (text, length, '(')) ||
          (last == ']' &&
           count_byte (text, length, ']') > count_byte (text, length, '[')) ||
          (last == '}' &&
           count_byte (text, length, '}') > count_byte (text, length, '{')))
        {
          length--;
          continue;
        }

      break;
    }

  return length;
}

static gboolean
append_url (GString    *out,
            const char *text,
            gsize      *consumed)
{
  g_autofree char *url = NULL;
  g_autofree char *escaped = NULL;
  gsize length;

  if (!starts_url (text))
    return FALSE;

  length = url_length (text);
  if (length <= strlen ("http://"))
    return FALSE;

  url = g_strndup (text, length);
  escaped = g_markup_escape_text (url, -1);
  g_string_append_printf (out, "<a href=\"%s\">%s</a>", escaped, escaped);
  *consumed = length;

  return TRUE;
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
               const char *text,
               gboolean    autolink)
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
              append_inline (out, inner, autolink);
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
                  append_inline (out, label, FALSE);
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
              append_inline (out, inner, autolink);
              g_string_append (out, "</i>");
              p = close + 1;
              continue;
            }
        }

      if (autolink)
        {
          gsize consumed = 0;

          if (append_url (out, p, &consumed))
            {
              p += consumed;
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

/*
 * CommonMark ATX headings need one to six hashes followed by whitespace or
 * end-of-line. Without that boundary, issue references such as "#1 fixed"
 * become giant headings.
 */
static gboolean
is_heading (const char *line)
{
  const char *text = line;
  guint level = 0;

  while (*text == '#')
    {
      level++;
      text++;
    }

  return level >= 1 && level <= 6 &&
         (*text == '\0' || *text == ' ' || *text == '\t');
}

/*
 * A heading, at a size that says which level it is.
 *
 * Bold alone made every heading in a long answer look like every emphasised
 * phrase in it, so a plan with sections read as one unbroken column of text.
 * Two sizes are enough: the top of a document, and everything under it.
 */
static void
append_heading (GString    *out,
                const char *line)
{
  const char *text = line;
  int level = 0;

  while (*text == '#')
    {
      level++;
      text++;
    }
  while (*text == ' ' || *text == '\t')
    text++;

  g_string_append_printf (out, "<span size=\"%s\"><b>",
                          level <= 2 ? "large" : "medium");
  append_inline (out, text, TRUE);
  g_string_append (out, "</b></span>");
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
           * state of affairs while a reply is still streaming.
           *
           * Monospace and a darker ground: a block of commands surrounded by
           * prose has to be seen to be one before it is read. */
          g_string_append (out, in_fence ? "</span></tt>"
                                         : "<tt><span background=\"#181818\">");
          in_fence = !in_fence;
          continue;
        }

      if (i > 0)
        g_string_append_c (out, '\n');

      if (in_fence)
        append_escaped (out, line, -1);
      else if (is_heading (line))
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
              append_inline (out, item + 2, TRUE);
            }
          else
            {
              append_inline (out, line, TRUE);
            }
        }
    }

  if (in_fence)
    g_string_append (out, "</span></tt>");

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

char *
xd_urls_to_pango (const char *text)
{
  g_autoptr (GString) out = g_string_new (NULL);
  const char *p = text != NULL ? text : "";

  while (*p != '\0')
    {
      gsize consumed = 0;

      if (append_url (out, p, &consumed))
        {
          p += consumed;
          continue;
        }

      append_escaped (out, p, 1);
      p++;
    }

  return g_string_free (g_steal_pointer (&out), FALSE);
}
