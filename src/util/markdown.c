#include "markdown.h"

#include <cmark.h>
#include <pango/pango.h>
#include <string.h>

static void
append_escaped (GString    *out,
                const char *text,
                gssize      length)
{
  g_autofree char *escaped =
    g_markup_escape_text (text != NULL ? text : "", length);

  g_string_append (out, escaped);
}

static gboolean
starts_url (const char *text)
{
  return g_str_has_prefix (text, "https://") ||
         g_str_has_prefix (text, "http://");
}

static gboolean
safe_link (const char *url)
{
  return starts_url (url != NULL ? url : "") ||
         g_str_has_prefix (url != NULL ? url : "", "mailto:");
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
 * cmark deliberately leaves bare URLs as text. Chats use them constantly, so
 * add links after parsing while preserving cmark's handling of every other
 * inline construct.
 */
static void
append_text (GString    *out,
             const char *text,
             gboolean    autolink)
{
  const char *at = text != NULL ? text : "";

  while (*at != '\0')
    {
      gsize consumed = 0;
      const char *next;

      if (autolink && append_url (out, at, &consumed))
        {
          at += consumed;
          continue;
        }

      next = g_utf8_next_char (at);
      append_escaped (out, at, next - at);
      at = next;
    }
}

static void render_inline_children (GString    *out,
                                    cmark_node *parent,
                                    gboolean    autolink);

static void
render_inline (GString    *out,
               cmark_node *node,
               gboolean    autolink)
{
  cmark_node_type type = cmark_node_get_type (node);

  switch (type)
    {
    case CMARK_NODE_TEXT:
      append_text (out, cmark_node_get_literal (node), autolink);
      break;

    case CMARK_NODE_SOFTBREAK:
    case CMARK_NODE_LINEBREAK:
      g_string_append_c (out, '\n');
      break;

    case CMARK_NODE_CODE:
      g_string_append (out, "<tt>");
      append_escaped (out, cmark_node_get_literal (node), -1);
      g_string_append (out, "</tt>");
      break;

    case CMARK_NODE_HTML_INLINE:
      /* Markdown must never become arbitrary Pango markup. */
      append_escaped (out, cmark_node_get_literal (node), -1);
      break;

    case CMARK_NODE_EMPH:
      g_string_append (out, "<i>");
      render_inline_children (out, node, autolink);
      g_string_append (out, "</i>");
      break;

    case CMARK_NODE_STRONG:
      g_string_append (out, "<b>");
      render_inline_children (out, node, autolink);
      g_string_append (out, "</b>");
      break;

    case CMARK_NODE_LINK:
      {
        const char *url = cmark_node_get_url (node);

        if (safe_link (url))
          {
            g_autofree char *href = g_markup_escape_text (url, -1);

            g_string_append_printf (out, "<a href=\"%s\">", href);
            render_inline_children (out, node, FALSE);
            g_string_append (out, "</a>");
          }
        else
          {
            render_inline_children (out, node, FALSE);
          }
      }
      break;

    case CMARK_NODE_IMAGE:
      {
        const char *url = cmark_node_get_url (node);

        g_string_append (out, "Image: ");
        if (safe_link (url))
          {
            g_autofree char *href = g_markup_escape_text (url, -1);

            g_string_append_printf (out, "<a href=\"%s\">", href);
          }
        render_inline_children (out, node, FALSE);
        if (safe_link (url))
          g_string_append (out, "</a>");
      }
      break;

    case CMARK_NODE_CUSTOM_INLINE:
      append_escaped (out, cmark_node_get_on_enter (node), -1);
      render_inline_children (out, node, autolink);
      append_escaped (out, cmark_node_get_on_exit (node), -1);
      break;

    default:
      render_inline_children (out, node, autolink);
      break;
    }
}

static void
render_inline_children (GString    *out,
                        cmark_node *parent,
                        gboolean    autolink)
{
  for (cmark_node *child = cmark_node_first_child (parent);
       child != NULL;
       child = cmark_node_next (child))
    render_inline (out, child, autolink);
}

/*
 * libcmark implements CommonMark, whose core syntax deliberately excludes
 * tables. Agent replies commonly use the GitHub table extension, though, and
 * showing its delimiter row as prose makes the result especially hard to
 * read. Recognise only complete, unambiguous table paragraphs here and keep
 * libcmark responsible for everything else.
 */
static void
append_inline_plain (GString    *out,
                     cmark_node *node)
{
  cmark_node_type type = cmark_node_get_type (node);

  switch (type)
    {
    case CMARK_NODE_TEXT:
    case CMARK_NODE_CODE:
    case CMARK_NODE_HTML_INLINE:
      g_string_append (out, cmark_node_get_literal (node));
      break;

    case CMARK_NODE_SOFTBREAK:
    case CMARK_NODE_LINEBREAK:
      g_string_append_c (out, '\n');
      break;

    default:
      for (cmark_node *child = cmark_node_first_child (node);
           child != NULL;
           child = cmark_node_next (child))
        append_inline_plain (out, child);
      break;
    }
}

static GPtrArray *
split_table_row (const char *line)
{
  g_autofree char *copy = g_strdup (line);
  g_auto (GStrv) pieces = NULL;
  GPtrArray *cells;
  char *contents;
  gsize length;

  contents = g_strstrip (copy);
  if (strchr (contents, '|') == NULL)
    return NULL;

  if (*contents == '|')
    contents++;

  length = strlen (contents);
  if (length > 0 && contents[length - 1] == '|')
    contents[length - 1] = '\0';

  pieces = g_strsplit (contents, "|", -1);
  cells = g_ptr_array_new_with_free_func (g_free);
  for (guint i = 0; pieces[i] != NULL; i++)
    g_ptr_array_add (cells, g_strdup (g_strstrip (pieces[i])));

  return cells;
}

static gboolean
is_table_delimiter (const char *cell)
{
  const char *at = cell;
  guint dashes = 0;

  if (*at == ':')
    at++;
  while (*at == '-')
    {
      dashes++;
      at++;
    }
  if (*at == ':')
    at++;

  return dashes >= 3 && *at == '\0';
}

static void
append_padding (GString *out,
                guint    count)
{
  while (count-- > 0)
    g_string_append_c (out, ' ');
}

static gboolean
render_table (GString    *out,
              cmark_node *paragraph)
{
  g_autoptr (GString) plain = g_string_new (NULL);
  g_auto (GStrv) lines = NULL;
  g_autoptr (GPtrArray) rows =
    g_ptr_array_new_with_free_func ((GDestroyNotify) g_ptr_array_unref);
  g_autoptr (GPtrArray) header = NULL;
  g_autoptr (GPtrArray) delimiter = NULL;
  g_autofree guint *widths = NULL;
  guint columns;

  for (cmark_node *child = cmark_node_first_child (paragraph);
       child != NULL;
       child = cmark_node_next (child))
    append_inline_plain (plain, child);

  lines = g_strsplit (plain->str, "\n", -1);
  if (lines[0] == NULL || lines[1] == NULL)
    return FALSE;

  header = split_table_row (lines[0]);
  delimiter = split_table_row (lines[1]);
  if (header == NULL || delimiter == NULL)
    return FALSE;

  columns = delimiter->len;
  if (columns == 0 || header->len != columns)
    return FALSE;

  for (guint column = 0; column < columns; column++)
    if (!is_table_delimiter (g_ptr_array_index (delimiter, column)))
      return FALSE;

  g_ptr_array_add (rows, g_steal_pointer (&header));

  for (guint line = 2; lines[line] != NULL; line++)
    {
      GPtrArray *row = split_table_row (lines[line]);

      if (row == NULL || row->len != columns)
        {
          g_clear_pointer (&row, g_ptr_array_unref);
          return FALSE;
        }
      g_ptr_array_add (rows, row);
    }

  widths = g_new0 (guint, columns);
  for (guint row = 0; row < rows->len; row++)
    {
      GPtrArray *cells = g_ptr_array_index (rows, row);

      for (guint column = 0; column < columns; column++)
        widths[column] = MAX (
          widths[column],
          (guint) g_utf8_strlen (g_ptr_array_index (cells, column), -1));
    }

  g_string_append (out, "<tt>");
  for (guint row = 0; row < rows->len; row++)
    {
      GPtrArray *cells = g_ptr_array_index (rows, row);

      if (row > 0)
        g_string_append_c (out, '\n');
      if (row == 0)
        g_string_append (out, "<b>");

      for (guint column = 0; column < columns; column++)
        {
          const char *cell = g_ptr_array_index (cells, column);
          guint width = (guint) g_utf8_strlen (cell, -1);

          if (column > 0)
            g_string_append (out, " \xe2\x94\x82 ");
          append_escaped (out, cell, -1);
          append_padding (out, widths[column] - width);
        }

      if (row == 0)
        {
          g_string_append (out, "</b>\n");
          for (guint column = 0; column < columns; column++)
            {
              if (column > 0)
                g_string_append (out, "\xe2\x94\x80\xe2\x94\xbc\xe2\x94\x80");
              for (guint i = 0; i < widths[column]; i++)
                g_string_append (out, "\xe2\x94\x80");
            }
        }
    }
  g_string_append (out, "</tt>");

  return TRUE;
}

static void render_block (GString    *out,
                          cmark_node *node,
                          guint       depth);

static void
render_block_children (GString    *out,
                       cmark_node *parent,
                       guint       depth,
                       const char *separator)
{
  for (cmark_node *child = cmark_node_first_child (parent);
       child != NULL;
       child = cmark_node_next (child))
    {
      g_autoptr (GString) block = g_string_new (NULL);

      render_block (block, child, depth);
      if (block->len == 0)
        continue;
      if (out->len > 0)
        g_string_append (out, separator);
      g_string_append_len (out, block->str, block->len);
    }
}

static void
append_prefixed_lines (GString    *out,
                       const char *text,
                       const char *first,
                       const char *rest)
{
  const char *line = text;

  g_string_append (out, first);
  while (*line != '\0')
    {
      const char *newline = strchr (line, '\n');

      if (newline == NULL)
        {
          g_string_append (out, line);
          break;
        }

      g_string_append_len (out, line, newline - line + 1);
      g_string_append (out, rest);
      line = newline + 1;
    }
}

static void
render_list (GString    *out,
             cmark_node *node,
             guint       depth)
{
  gboolean ordered =
    cmark_node_get_list_type (node) == CMARK_ORDERED_LIST;
  int number = ordered ? cmark_node_get_list_start (node) : 1;
  guint index = 0;

  for (cmark_node *item = cmark_node_first_child (node);
       item != NULL;
       item = cmark_node_next (item), index++, number++)
    {
      g_autoptr (GString) contents = g_string_new (NULL);
      g_autofree char *indent = g_strnfill (depth * 2, ' ');
      g_autofree char *marker =
        ordered ? g_strdup_printf ("%d. ", number) : g_strdup ("\xe2\x80\xa2 ");
      g_autofree char *first = g_strconcat (indent, marker, NULL);
      g_autofree char *rest =
        g_strnfill (strlen (indent) + g_utf8_strlen (marker, -1), ' ');

      render_block_children (contents, item, depth + 1, "\n");
      if (contents->len == 0)
        continue;
      if (index > 0)
        g_string_append_c (out, '\n');
      append_prefixed_lines (out, contents->str, first, rest);
    }
}

static void
render_block (GString    *out,
              cmark_node *node,
              guint       depth)
{
  cmark_node_type type = cmark_node_get_type (node);

  switch (type)
    {
    case CMARK_NODE_DOCUMENT:
      render_block_children (out, node, depth, "\n\n");
      break;

    case CMARK_NODE_PARAGRAPH:
      if (!render_table (out, node))
        render_inline_children (out, node, TRUE);
      break;

    case CMARK_NODE_HEADING:
      g_string_append_printf (
        out, "<span size=\"%s\"><b>",
        cmark_node_get_heading_level (node) <= 2 ? "large" : "medium");
      render_inline_children (out, node, TRUE);
      g_string_append (out, "</b></span>");
      break;

    case CMARK_NODE_BLOCK_QUOTE:
      {
        g_autoptr (GString) contents = g_string_new (NULL);

        render_block_children (contents, node, depth, "\n\n");
        append_prefixed_lines (out, contents->str, "\xe2\x94\x82 ",
                               "\xe2\x94\x82 ");
      }
      break;

    case CMARK_NODE_LIST:
      render_list (out, node, depth);
      break;

    case CMARK_NODE_ITEM:
      render_block_children (out, node, depth + 1, "\n");
      break;

    case CMARK_NODE_CODE_BLOCK:
      {
        const char *literal = cmark_node_get_literal (node);
        gsize length = literal != NULL ? strlen (literal) : 0;

        if (length > 0 && literal[length - 1] == '\n')
          length--;
        g_string_append (out, "<tt><span background=\"#181818\">");
        append_escaped (out, literal, length);
        g_string_append (out, "</span></tt>");
      }
      break;

    case CMARK_NODE_HTML_BLOCK:
      {
        const char *literal = cmark_node_get_literal (node);
        gsize length = literal != NULL ? strlen (literal) : 0;

        if (length > 0 && literal[length - 1] == '\n')
          length--;
        append_escaped (out, literal, length);
      }
      break;

    case CMARK_NODE_THEMATIC_BREAK:
      g_string_append (out, "\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
                            "\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
                            "\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80");
      break;

    case CMARK_NODE_CUSTOM_BLOCK:
      append_escaped (out, cmark_node_get_on_enter (node), -1);
      render_block_children (out, node, depth, "\n\n");
      append_escaped (out, cmark_node_get_on_exit (node), -1);
      break;

    default:
      render_inline (out, node, TRUE);
      break;
    }
}

static gboolean
valid_pango_markup (const char *markup)
{
  g_autoptr (GString) check = g_string_new (markup);
  const char *open;

  /* GtkLabel adds links on top of Pango markup; Pango's validator does not
   * know the <a> tag, so remove only those wrappers for validation. */
  while ((open = strstr (check->str, "<a href=\"")) != NULL)
    {
      const char *end = strchr (open, '>');

      if (end == NULL)
        return FALSE;
      g_string_erase (check, open - check->str, end - open + 1);
    }

  while ((open = strstr (check->str, "</a>")) != NULL)
    g_string_erase (check, open - check->str, strlen ("</a>"));

  return pango_parse_markup (
    check->str, -1, 0, NULL, NULL, NULL, NULL);
}

char *
xd_markdown_to_pango (const char *text)
{
  cmark_node *document;
  g_autoptr (GString) out = g_string_new (NULL);

  if (text == NULL || *text == '\0')
    return g_strdup ("");

  document = cmark_parse_document (
    text, strlen (text), CMARK_OPT_VALIDATE_UTF8);
  if (document == NULL)
    return g_markup_escape_text (text, -1);

  render_block (out, document, 0);
  cmark_node_free (document);

  if (!valid_pango_markup (out->str))
    {
      g_debug ("markdown produced invalid markup; falling back to plain text");
      return g_markup_escape_text (text, -1);
    }

  return g_string_free (g_steal_pointer (&out), FALSE);
}

char *
xd_urls_to_pango (const char *text)
{
  g_autoptr (GString) out = g_string_new (NULL);

  append_text (out, text, TRUE);
  return g_string_free (g_steal_pointer (&out), FALSE);
}
