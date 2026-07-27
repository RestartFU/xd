#include "diff-view.h"

#include "util/unified-diff.h"

#define INLINE_EAGER_ROWS 60
#define INLINE_PREVIEW_ROWS 120
#define PANE_PREVIEW_ROWS 250

static void
clear_box (GtkBox *box)
{
  GtkWidget *child;

  while ((child = gtk_widget_get_first_child (GTK_WIDGET (box))) != NULL)
    gtk_box_remove (box, child);
}

static GtkWidget *
diff_label (const char *markup)
{
  GtkWidget *label = gtk_label_new (NULL);

  gtk_label_set_markup (GTK_LABEL (label), markup);
  gtk_label_set_selectable (GTK_LABEL (label), TRUE);
  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_label_set_yalign (GTK_LABEL (label), 0.0f);
  gtk_widget_set_hexpand (label, TRUE);
  gtk_widget_add_css_class (label, "xd-diff-text");
  return label;
}

static void
fill_parsed_rows (GtkBox    *box,
                  GPtrArray *lines,
                  gboolean   show_file_headers,
                  guint      limit)
{
  g_autofree char *markup = NULL;

  clear_box (box);
  markup = xd_unified_diff_markup (
    lines, show_file_headers, limit);
  gtk_box_append (box, diff_label (markup));
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

  g_return_if_fail (GTK_IS_BOX (box));

  lines = xd_unified_diff_parse (patch, additions, deletions);
  fill_parsed_rows (box, lines, show_file_headers, limit);
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
  rows = xd_unified_diff_display_rows (lines, show_file_headers);
  if (additions != NULL)
    *additions = added;
  if (deletions != NULL)
    *deletions = removed;

  if (!show_file_headers || rows <= INLINE_EAGER_ROWS)
    {
      fill_parsed_rows (GTK_BOX (box), lines, show_file_headers,
                        INLINE_PREVIEW_ROWS);
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
