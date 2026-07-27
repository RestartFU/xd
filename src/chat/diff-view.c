#include "diff-view.h"

#include "util/unified-diff.h"

#include <string.h>

#define INLINE_EAGER_ROWS 60
#define INLINE_PREVIEW_ROWS 120
#define PANE_PREVIEW_ROWS 250
#define DISPLAY_LINE_BYTES 4096

static void
clear_box (GtkBox *box)
{
  GtkWidget *child;

  while ((child = gtk_widget_get_first_child (GTK_WIDGET (box))) != NULL)
    gtk_box_remove (box, child);
}

static char *
display_text (const char *text)
{
  gsize length;
  gsize cut;

  if (text == NULL)
    return g_strdup ("");

  length = strlen (text);
  if (length <= DISPLAY_LINE_BYTES)
    return g_strdup (text);

  cut = DISPLAY_LINE_BYTES;
  while (cut > 0 && (((guchar) text[cut]) & 0xc0) == 0x80)
    cut--;

  return g_strdup_printf ("%.*s…", (int) cut, text);
}

static GtkWidget *
line_number (guint number)
{
  g_autofree char *text =
    number > 0 ? g_strdup_printf ("%u", number) : NULL;
  GtkWidget *label = gtk_label_new (text);

  gtk_label_set_xalign (GTK_LABEL (label), 1.0f);
  gtk_label_set_width_chars (GTK_LABEL (label), 4);
  gtk_widget_add_css_class (label, "xd-diff-gutter");
  gtk_widget_add_css_class (label, "dim-label");
  return label;
}

static GtkWidget *
code_label (const char *text)
{
  g_autofree char *shown = display_text (text);
  GtkWidget *label = gtk_label_new (shown);

  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_label_set_selectable (GTK_LABEL (label), TRUE);
  gtk_label_set_single_line_mode (GTK_LABEL (label), TRUE);
  gtk_widget_set_hexpand (label, TRUE);
  gtk_widget_add_css_class (label, "xd-diff-code");
  return label;
}

static void
append_file (GtkBox           *box,
             const XdDiffLine *line,
             guint             additions,
             guint             deletions)
{
  GtkWidget *row = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 7);
  GtkWidget *icon =
    gtk_image_new_from_icon_name ("text-x-generic-symbolic");
  GtkWidget *path = code_label (line->text);
  g_autofree char *stats_text =
    additions + deletions > 0
      ? g_strdup_printf ("+%u  −%u", additions, deletions) : NULL;
  GtkWidget *stats = gtk_label_new (stats_text);

  gtk_widget_add_css_class (path, "heading");
  gtk_widget_add_css_class (icon, "dim-label");
  gtk_widget_add_css_class (stats, "caption");
  gtk_widget_add_css_class (stats, "dim-label");
  gtk_box_append (GTK_BOX (row), icon);
  gtk_box_append (GTK_BOX (row), path);
  gtk_box_append (GTK_BOX (row), stats);
  gtk_widget_add_css_class (row, "xd-diff-file");
  gtk_box_append (box, row);
}

static void
append_full_line (GtkBox           *box,
                  const XdDiffLine *line)
{
  GtkWidget *row = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);
  const char *class_name =
    line->kind == XD_DIFF_LINE_HUNK ? "xd-diff-hunk" : "xd-diff-meta";

  gtk_box_append (GTK_BOX (row), code_label (line->text));
  gtk_widget_add_css_class (row, "xd-diff-line");
  gtk_widget_add_css_class (row, class_name);
  gtk_box_append (box, row);
}

static void
append_code_line (GtkBox           *box,
                  const XdDiffLine *line)
{
  GtkWidget *row = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);
  GtkWidget *marker;
  const char *marker_text = " ";

  if (line->kind == XD_DIFF_LINE_REMOVED)
    {
      marker_text = "−";
      gtk_widget_add_css_class (row, "xd-diff-removed");
    }
  else if (line->kind == XD_DIFF_LINE_ADDED)
    {
      marker_text = "+";
      gtk_widget_add_css_class (row, "xd-diff-added");
    }
  else
    {
      gtk_widget_add_css_class (row, "xd-diff-context");
    }

  gtk_box_append (GTK_BOX (row), line_number (line->old_line));
  gtk_box_append (GTK_BOX (row), line_number (line->new_line));
  marker = gtk_label_new (marker_text);
  gtk_label_set_width_chars (GTK_LABEL (marker), 2);
  gtk_widget_add_css_class (marker, "xd-diff-marker");
  gtk_box_append (GTK_BOX (row), marker);
  gtk_box_append (GTK_BOX (row), code_label (line->text));
  gtk_widget_set_hexpand (row, TRUE);
  gtk_widget_add_css_class (row, "xd-diff-line");
  gtk_widget_add_css_class (row, "xd-diff-code-row");
  gtk_box_append (box, row);
}

static guint
display_row_count (GPtrArray *lines,
                   gboolean   show_file_headers)
{
  guint rows = 0;

  for (guint i = 0; i < lines->len; i++)
    {
      XdDiffLine *line = g_ptr_array_index (lines, i);

      if (line->kind != XD_DIFF_LINE_FILE || show_file_headers)
        rows++;
    }

  return rows;
}

static void
append_truncation (GtkBox *box,
                   guint   rendered,
                   guint   total)
{
  g_autofree char *text = g_strdup_printf (
    "Showing first %u of %u rows", rendered, total);
  GtkWidget *label = gtk_label_new (text);

  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_widget_add_css_class (label, "caption");
  gtk_widget_add_css_class (label, "dim-label");
  gtk_widget_add_css_class (label, "xd-diff-truncated");
  gtk_box_append (box, label);
}

static void
fill_rows (GtkBox     *box,
           const char *patch,
           gboolean    show_file_headers,
           guint       limit,
           guint      *additions,
           guint      *deletions)
{
  g_autoptr (GPtrArray) lines = NULL;
  guint rendered = 0;
  guint total;

  g_return_if_fail (GTK_IS_BOX (box));

  clear_box (box);
  lines = xd_unified_diff_parse (patch, additions, deletions);
  total = display_row_count (lines, show_file_headers);

  for (guint i = 0;
       i < lines->len && (limit == 0 || rendered < limit); )
    {
      XdDiffLine *line = g_ptr_array_index (lines, i);

      switch (line->kind)
        {
        case XD_DIFF_LINE_FILE:
          if (show_file_headers)
            {
              guint file_additions = 0;
              guint file_deletions = 0;

              for (guint j = i + 1; j < lines->len; j++)
                {
                  XdDiffLine *within = g_ptr_array_index (lines, j);

                  if (within->kind == XD_DIFF_LINE_FILE)
                    break;
                  if (within->kind == XD_DIFF_LINE_ADDED)
                    file_additions++;
                  else if (within->kind == XD_DIFF_LINE_REMOVED)
                    file_deletions++;
                }

              append_file (
                box, line, file_additions, file_deletions);
              rendered++;
            }
          i++;
          break;
        case XD_DIFF_LINE_HUNK:
        case XD_DIFF_LINE_META:
          append_full_line (box, line);
          rendered++;
          i++;
          break;
        case XD_DIFF_LINE_CONTEXT:
        case XD_DIFF_LINE_REMOVED:
        case XD_DIFF_LINE_ADDED:
          append_code_line (box, line);
          rendered++;
          i++;
          break;
        }
    }

  if (rendered < total)
    append_truncation (box, rendered, total);
}

void
xd_diff_view_fill (GtkBox     *box,
                   const char *patch,
                   gboolean    show_file_headers,
                   guint      *additions,
                   guint      *deletions)
{
  fill_rows (box, patch, show_file_headers, PANE_PREVIEW_ROWS,
             additions, deletions);
}

static void
on_large_diff_expanded (GtkExpander *expander,
                        GParamSpec  *pspec,
                        gpointer     user_data)
{
  GtkWidget *box;
  const char *patch;

  if (!gtk_expander_get_expanded (expander) ||
      g_object_get_data (G_OBJECT (expander), "xd-diff-loaded") != NULL)
    return;

  box = gtk_expander_get_child (expander);
  patch = g_object_get_data (G_OBJECT (expander), "xd-diff-patch");
  fill_rows (GTK_BOX (box), patch, TRUE, INLINE_PREVIEW_ROWS, NULL, NULL);
  g_object_set_data (G_OBJECT (expander), "xd-diff-loaded",
                     GINT_TO_POINTER (TRUE));
}

GtkWidget *
xd_diff_view_new (const char *patch,
                  gboolean    show_file_headers,
                  guint      *additions,
                  guint      *deletions)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  g_autoptr (GPtrArray) lines = NULL;
  guint added = 0;
  guint removed = 0;
  guint rows;

  gtk_widget_set_valign (box, GTK_ALIGN_START);
  gtk_widget_set_hexpand (box, TRUE);
  gtk_widget_add_css_class (box, "xd-diff-view");

  lines = xd_unified_diff_parse (patch, &added, &removed);
  rows = display_row_count (lines, show_file_headers);
  if (additions != NULL)
    *additions = added;
  if (deletions != NULL)
    *deletions = removed;

  if (!show_file_headers || rows <= INLINE_EAGER_ROWS)
    {
      fill_rows (GTK_BOX (box), patch, show_file_headers,
                 INLINE_PREVIEW_ROWS, NULL, NULL);
    }
  else
    {
      g_autofree char *summary = g_strdup_printf (
        "Large diff · %u rows · +%u  −%u", rows, added, removed);
      GtkWidget *expander = gtk_expander_new (summary);
      GtkWidget *preview = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);

      gtk_expander_set_child (GTK_EXPANDER (expander), preview);
      gtk_widget_add_css_class (expander, "xd-diff-expander");
      g_object_set_data_full (G_OBJECT (expander), "xd-diff-patch",
                              g_strdup (patch), g_free);
      g_signal_connect (expander, "notify::expanded",
                        G_CALLBACK (on_large_diff_expanded), NULL);
      gtk_box_append (GTK_BOX (box), expander);
    }

  return box;
}
