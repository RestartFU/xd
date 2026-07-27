#include "unified-diff.h"

#include <stdlib.h>
#include <string.h>

#define DISPLAY_LINE_BYTES 1024

void
xd_diff_line_free (XdDiffLine *line)
{
  if (line == NULL)
    return;

  g_free (line->text);
  g_free (line);
}

static gboolean
parse_hunk_start (const char *line,
                  guint      *old_line,
                  guint      *new_line)
{
  const char *old_at;
  const char *new_at;
  char *end = NULL;
  unsigned long old_value;
  unsigned long new_value;

  if (line == NULL || !g_str_has_prefix (line, "@@ -"))
    return FALSE;

  old_at = line + strlen ("@@ -");
  old_value = strtoul (old_at, &end, 10);
  if (end == old_at)
    return FALSE;

  new_at = strstr (end, " +");
  if (new_at == NULL)
    return FALSE;
  new_at += 2;

  new_value = strtoul (new_at, &end, 10);
  if (end == new_at || strstr (end, " @@") == NULL)
    return FALSE;

  *old_line = (guint) MIN (old_value, G_MAXUINT);
  *new_line = (guint) MIN (new_value, G_MAXUINT);
  return TRUE;
}

static gboolean
is_plumbing_header (const char *line)
{
  return g_str_has_prefix (line, "index ") ||
         g_str_has_prefix (line, "--- ") ||
         g_str_has_prefix (line, "+++ ");
}

static char *
file_title (const char *line)
{
  const char *path = NULL;

  /* The target path is the useful identity for adds and renames. Git quotes
   * unusual paths; keep its escaping, but remove the surrounding plumbing. */
  for (const char *at = line; (at = strstr (at, " b/")) != NULL; at += 3)
    path = at + 3;

  if (path != NULL)
    return g_strdup (path);

  for (const char *at = line; (at = strstr (at, " \"b/")) != NULL; at += 4)
    path = at + 4;

  if (path != NULL)
    {
      char *title = g_strdup (path);

      if (g_str_has_suffix (title, "\""))
        title[strlen (title) - 1] = '\0';
      return title;
    }

  return g_strdup (line + strlen ("diff --git "));
}

static void
append_line (GPtrArray      *lines,
             XdDiffLineKind  kind,
             const char     *text,
             guint           old_line,
             guint           new_line)
{
  XdDiffLine *line = g_new0 (XdDiffLine, 1);

  line->kind = kind;
  line->text = g_strdup (text);
  line->old_line = old_line;
  line->new_line = new_line;
  g_ptr_array_add (lines, line);
}

GPtrArray *
xd_unified_diff_parse (const char *patch,
                       guint      *additions,
                       guint      *deletions)
{
  g_autoptr (GPtrArray) result =
    g_ptr_array_new_with_free_func ((GDestroyNotify) xd_diff_line_free);
  g_auto (GStrv) raw_lines = NULL;
  guint old_line = 0;
  guint new_line = 0;
  guint added = 0;
  guint removed = 0;
  gboolean in_hunk = FALSE;

  raw_lines = g_strsplit (patch != NULL ? patch : "", "\n", -1);

  for (gsize i = 0; raw_lines[i] != NULL; i++)
    {
      const char *raw = raw_lines[i];

      if (g_str_has_prefix (raw, "diff --git "))
        {
          g_autofree char *title = file_title (raw);

          append_line (result, XD_DIFF_LINE_FILE, title, 0, 0);
          in_hunk = FALSE;
          continue;
        }

      if (parse_hunk_start (raw, &old_line, &new_line))
        {
          append_line (result, XD_DIFF_LINE_HUNK, raw, old_line, new_line);
          in_hunk = TRUE;
          continue;
        }

      if (!in_hunk)
        {
          if (*raw == '\0' || is_plumbing_header (raw))
            continue;

          append_line (result, XD_DIFF_LINE_META, raw, 0, 0);
          continue;
        }

      if (raw[0] == '+')
        {
          append_line (result, XD_DIFF_LINE_ADDED, raw + 1, 0, new_line++);
          added++;
        }
      else if (raw[0] == '-')
        {
          append_line (result, XD_DIFF_LINE_REMOVED, raw + 1, old_line++, 0);
          removed++;
        }
      else if (raw[0] == ' ')
        {
          append_line (result, XD_DIFF_LINE_CONTEXT, raw + 1,
                       old_line++, new_line++);
        }
      else if (*raw != '\0')
        {
          /* "\ No newline at end of file" and any future Git annotation. */
          append_line (result, XD_DIFF_LINE_META, raw, 0, 0);
        }
    }

  if (additions != NULL)
    *additions = added;
  if (deletions != NULL)
    *deletions = removed;

  return g_steal_pointer (&result);
}

guint
xd_unified_diff_display_rows (GPtrArray *lines,
                              gboolean   show_file_headers)
{
  guint rows = 0;

  g_return_val_if_fail (lines != NULL, 0);

  for (guint i = 0; i < lines->len; i++)
    {
      XdDiffLine *line = g_ptr_array_index (lines, i);

      if (line->kind != XD_DIFF_LINE_FILE || show_file_headers)
        rows++;
    }

  return rows;
}

static char *
display_text (const char *text)
{
  g_autofree char *valid = g_utf8_make_valid (text != NULL ? text : "", -1);
  gsize length = strlen (valid);
  gsize cut;

  if (length <= DISPLAY_LINE_BYTES)
    return g_steal_pointer (&valid);

  cut = DISPLAY_LINE_BYTES;
  while (cut > 0 && (((guchar) valid[cut]) & 0xc0) == 0x80)
    cut--;

  return g_strdup_printf ("%.*s…", (int) cut, valid);
}

static char *
escaped_display_text (const char *text)
{
  g_autofree char *shown = display_text (text);

  return g_markup_escape_text (shown, -1);
}

static void
start_markup_row (GString *markup,
                  guint    rendered)
{
  if (rendered > 0)
    g_string_append_c (markup, '\n');
}

static void
append_file_markup (GString          *markup,
                    GPtrArray        *lines,
                    guint             position,
                    const XdDiffLine *line,
                    guint             rendered)
{
  g_autofree char *path = escaped_display_text (line->text);
  guint additions = 0;
  guint deletions = 0;

  for (guint i = position + 1; i < lines->len; i++)
    {
      XdDiffLine *within = g_ptr_array_index (lines, i);

      if (within->kind == XD_DIFF_LINE_FILE)
        break;
      if (within->kind == XD_DIFF_LINE_ADDED)
        additions++;
      else if (within->kind == XD_DIFF_LINE_REMOVED)
        deletions++;
    }

  start_markup_row (markup, rendered);
  g_string_append_printf (
    markup,
    "<span weight=\"bold\">%s</span>"
    "  <span foreground=\"#57e389\">+%u</span>"
    "  <span foreground=\"#f66151\">−%u</span>",
    path, additions, deletions);
}

static void
append_full_line_markup (GString          *markup,
                         const XdDiffLine *line,
                         guint             rendered)
{
  g_autofree char *text = escaped_display_text (line->text);

  start_markup_row (markup, rendered);
  if (line->kind == XD_DIFF_LINE_HUNK)
    g_string_append_printf (
      markup, "<span foreground=\"#78aeed\">%s</span>", text);
  else
    g_string_append_printf (
      markup, "<span foreground=\"#9a9996\">%s</span>", text);
}

static void
append_code_line_markup (GString          *markup,
                         const XdDiffLine *line,
                         guint             rendered)
{
  g_autofree char *text = escaped_display_text (line->text);
  g_autofree char *old_line =
    line->old_line > 0 ? g_strdup_printf ("%4u", line->old_line)
                       : g_strdup ("    ");
  g_autofree char *new_line =
    line->new_line > 0 ? g_strdup_printf ("%4u", line->new_line)
                       : g_strdup ("    ");
  const char *marker = " ";
  const char *colour = "#9a9996";
  const char *background = NULL;

  if (line->kind == XD_DIFF_LINE_ADDED)
    {
      marker = "+";
      colour = "#57e389";
      background = "#183522";
    }
  else if (line->kind == XD_DIFF_LINE_REMOVED)
    {
      marker = "−";
      colour = "#f66151";
      background = "#3a1d1b";
    }

  start_markup_row (markup, rendered);
  if (background != NULL)
    g_string_append_printf (
      markup,
      "<span background=\"%s\">"
      "<span foreground=\"#c0bfbc\">%s %s</span>"
      " <span foreground=\"%s\" weight=\"bold\">%s</span>"
      " <span foreground=\"%s\">%s</span>"
      "</span>",
      background, old_line, new_line, colour, marker, colour, text);
  else
    g_string_append_printf (
      markup,
      "<span foreground=\"#77767b\">%s %s</span>"
      " <span foreground=\"%s\" weight=\"bold\">%s</span> %s",
      old_line, new_line, colour, marker, text);
}

char *
xd_unified_diff_markup (GPtrArray *lines,
                        gboolean   show_file_headers,
                        guint      limit)
{
  g_autoptr (GString) markup = g_string_new (NULL);
  guint rendered = 0;
  guint total;

  g_return_val_if_fail (lines != NULL, g_strdup (""));

  total = xd_unified_diff_display_rows (lines, show_file_headers);
  for (guint i = 0;
       i < lines->len && (limit == 0 || rendered < limit);
       i++)
    {
      XdDiffLine *line = g_ptr_array_index (lines, i);

      if (line->kind == XD_DIFF_LINE_FILE)
        {
          if (!show_file_headers)
            continue;
          append_file_markup (markup, lines, i, line, rendered);
        }
      else if (line->kind == XD_DIFF_LINE_HUNK ||
               line->kind == XD_DIFF_LINE_META)
        {
          append_full_line_markup (markup, line, rendered);
        }
      else
        {
          append_code_line_markup (markup, line, rendered);
        }

      rendered++;
    }

  if (rendered < total)
    {
      start_markup_row (markup, rendered);
      g_string_append_printf (
        markup,
        "<span foreground=\"#9a9996\">Showing first %u of %u rows</span>",
        rendered, total);
    }

  return g_string_free (g_steal_pointer (&markup), FALSE);
}
