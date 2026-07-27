#include "diff-view.h"

#include "util/unified-diff.h"

static void
clear_box (GtkBox *box)
{
  GtkWidget *child;

  while ((child = gtk_widget_get_first_child (GTK_WIDGET (box))) != NULL)
    gtk_box_remove (box, child);
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
  GtkWidget *label = gtk_label_new (text != NULL ? text : "");

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

static GtkWidget *
side (const XdDiffLine *line,
      gboolean          old_side)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);
  GtkWidget *marker;
  const char *marker_text = " ";
  guint number = 0;

  if (line != NULL)
    {
      number = old_side ? line->old_line : line->new_line;
      if (line->kind == XD_DIFF_LINE_REMOVED)
        {
          marker_text = "−";
          gtk_widget_add_css_class (box, "xd-diff-removed");
        }
      else if (line->kind == XD_DIFF_LINE_ADDED)
        {
          marker_text = "+";
          gtk_widget_add_css_class (box, "xd-diff-added");
        }
      else
        {
          gtk_widget_add_css_class (box, "xd-diff-context");
        }
    }
  else
    {
      gtk_widget_add_css_class (box, "xd-diff-empty");
    }

  gtk_box_append (GTK_BOX (box), line_number (number));
  marker = gtk_label_new (marker_text);
  gtk_label_set_width_chars (GTK_LABEL (marker), 2);
  gtk_widget_add_css_class (marker, "xd-diff-marker");
  gtk_box_append (GTK_BOX (box), marker);
  gtk_box_append (GTK_BOX (box), code_label (
    line != NULL ? line->text : ""));

  gtk_widget_set_hexpand (box, TRUE);
  gtk_widget_add_css_class (box, "xd-diff-side");
  if (!old_side)
    gtk_widget_add_css_class (box, "xd-diff-new-side");
  return box;
}

static void
append_pair (GtkBox           *box,
             const XdDiffLine *old_line,
             const XdDiffLine *new_line)
{
  GtkWidget *row = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);

  gtk_box_set_homogeneous (GTK_BOX (row), TRUE);
  gtk_box_append (GTK_BOX (row), side (old_line, TRUE));
  gtk_box_append (GTK_BOX (row), side (new_line, FALSE));
  gtk_widget_add_css_class (row, "xd-diff-line");
  gtk_box_append (box, row);
}

static guint
append_changes (GtkBox    *box,
                GPtrArray *lines,
                guint      start)
{
  g_autoptr (GPtrArray) removed = g_ptr_array_new ();
  g_autoptr (GPtrArray) added = g_ptr_array_new ();
  guint at = start;

  while (at < lines->len)
    {
      XdDiffLine *line = g_ptr_array_index (lines, at);

      if (line->kind == XD_DIFF_LINE_REMOVED)
        g_ptr_array_add (removed, line);
      else if (line->kind == XD_DIFF_LINE_ADDED)
        g_ptr_array_add (added, line);
      else
        break;
      at++;
    }

  for (guint i = 0; i < MAX (removed->len, added->len); i++)
    append_pair (box,
                 i < removed->len ? g_ptr_array_index (removed, i) : NULL,
                 i < added->len ? g_ptr_array_index (added, i) : NULL);

  return at;
}

void
xd_diff_view_fill (GtkBox     *box,
                   const char *patch,
                   gboolean    show_file_headers,
                   guint      *additions,
                   guint      *deletions)
{
  g_autoptr (GPtrArray) lines = NULL;

  g_return_if_fail (GTK_IS_BOX (box));

  clear_box (box);
  lines = xd_unified_diff_parse (patch, additions, deletions);
  for (guint i = 0; i < lines->len; )
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
            }
          i++;
          break;
        case XD_DIFF_LINE_HUNK:
        case XD_DIFF_LINE_META:
          append_full_line (box, line);
          i++;
          break;
        case XD_DIFF_LINE_CONTEXT:
          append_pair (box, line, line);
          i++;
          break;
        case XD_DIFF_LINE_REMOVED:
        case XD_DIFF_LINE_ADDED:
          i = append_changes (box, lines, i);
          break;
        }
    }
}

GtkWidget *
xd_diff_view_new (const char *patch,
                  gboolean    show_file_headers,
                  guint      *additions,
                  guint      *deletions)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);

  gtk_widget_set_valign (box, GTK_ALIGN_START);
  gtk_widget_set_hexpand (box, TRUE);
  gtk_widget_add_css_class (box, "xd-diff-view");
  xd_diff_view_fill (
    GTK_BOX (box), patch, show_file_headers, additions, deletions);
  return box;
}
