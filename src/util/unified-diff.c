#include "unified-diff.h"

#include <stdlib.h>
#include <string.h>

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
  return g_str_has_prefix (line, "diff --git ") ||
         g_str_has_prefix (line, "index ") ||
         g_str_has_prefix (line, "--- ") ||
         g_str_has_prefix (line, "+++ ");
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
