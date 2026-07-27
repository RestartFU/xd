#include "diff-view.h"

#include "util/unified-diff.h"

#define INLINE_EAGER_ROWS 60
#define INLINE_PREVIEW_ROWS 120
#define VIRTUAL_DIFF_ROWS 80

typedef struct
{
  GPtrArray *lines;
  gboolean show_file_headers;
} VirtualDiff;

static void
virtual_diff_free (VirtualDiff *diff)
{
  g_clear_pointer (&diff->lines, g_ptr_array_unref);
  g_free (diff);
}

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
  fill_rows (box, patch, show_file_headers, 0,
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

static void
setup_virtual_chunk (GtkSignalListItemFactory *factory,
                     GtkListItem              *item,
                     gpointer                  user_data)
{
  GtkWidget *label = diff_label ("");

  gtk_widget_add_css_class (label, "xd-diff-chunk");
  gtk_list_item_set_child (item, label);
}

static void
bind_virtual_chunk (GtkSignalListItemFactory *factory,
                    GtkListItem              *item,
                    gpointer                  user_data)
{
  VirtualDiff *diff = user_data;
  GtkStringObject *descriptor =
    GTK_STRING_OBJECT (gtk_list_item_get_item (item));
  const char *value = gtk_string_object_get_string (descriptor);
  char *at = NULL;
  guint64 start = g_ascii_strtoull (value, &at, 10);
  guint64 end = at != NULL && *at == ':'
    ? g_ascii_strtoull (at + 1, NULL, 10) : start;
  g_autofree char *markup = xd_unified_diff_markup_slice (
    diff->lines, diff->show_file_headers,
    (guint) MIN (start, G_MAXUINT), (guint) MIN (end, G_MAXUINT));

  gtk_label_set_markup (
    GTK_LABEL (gtk_list_item_get_child (item)), markup);
}

static void
unbind_virtual_chunk (GtkSignalListItemFactory *factory,
                      GtkListItem              *item,
                      gpointer                  user_data)
{
  gtk_label_set_text (GTK_LABEL (gtk_list_item_get_child (item)), "");
}

GtkWidget *
xd_diff_view_new_virtualized (void)
{
  g_autoptr (GtkListItemFactory) factory =
    gtk_signal_list_item_factory_new ();
  GtkWidget *view = gtk_list_view_new (NULL, NULL);
  VirtualDiff *diff = g_new0 (VirtualDiff, 1);

  diff->lines =
    g_ptr_array_new_with_free_func ((GDestroyNotify) xd_diff_line_free);
  g_object_set_data_full (
    G_OBJECT (factory), "xd-virtual-diff", diff,
    (GDestroyNotify) virtual_diff_free);
  g_signal_connect (factory, "setup",
                    G_CALLBACK (setup_virtual_chunk), diff);
  g_signal_connect (factory, "bind",
                    G_CALLBACK (bind_virtual_chunk), diff);
  g_signal_connect (factory, "unbind",
                    G_CALLBACK (unbind_virtual_chunk), diff);

  gtk_list_view_set_factory (
    GTK_LIST_VIEW (view), GTK_LIST_ITEM_FACTORY (factory));
  gtk_widget_add_css_class (view, "xd-diff-list");

  return view;
}

void
xd_diff_view_fill_virtualized (GtkListView *view,
                               const char  *patch,
                               gboolean     show_file_headers,
                               guint       *additions,
                               guint       *deletions)
{
  GtkListItemFactory *factory;
  VirtualDiff *diff;
  g_autoptr (GtkStringList) chunks = gtk_string_list_new (NULL);
  g_autoptr (GtkNoSelection) selection = NULL;
  guint start = 0;
  guint rows = 0;

  g_return_if_fail (GTK_IS_LIST_VIEW (view));

  factory = gtk_list_view_get_factory (view);
  diff = g_object_get_data (G_OBJECT (factory), "xd-virtual-diff");

  /* Unbind old rows before replacing the parsed data they reference. */
  gtk_list_view_set_model (view, NULL);
  g_clear_pointer (&diff->lines, g_ptr_array_unref);
  diff->lines = xd_unified_diff_parse (patch, additions, deletions);
  diff->show_file_headers = show_file_headers;

  for (guint i = 0; i < diff->lines->len; i++)
    {
      XdDiffLine *line = g_ptr_array_index (diff->lines, i);

      if (line->kind == XD_DIFF_LINE_FILE && !show_file_headers)
        continue;

      if (rows == 0)
        start = i;
      rows++;

      if (rows == VIRTUAL_DIFF_ROWS)
        {
          g_autofree char *descriptor =
            g_strdup_printf ("%u:%u", start, i + 1);

          gtk_string_list_append (chunks, descriptor);
          rows = 0;
        }
    }

  if (rows > 0)
    {
      g_autofree char *descriptor =
        g_strdup_printf ("%u:%u", start, diff->lines->len);

      gtk_string_list_append (chunks, descriptor);
    }

  selection = gtk_no_selection_new (
    G_LIST_MODEL (g_object_ref (chunks)));
  gtk_list_view_set_model (view, GTK_SELECTION_MODEL (selection));
}
