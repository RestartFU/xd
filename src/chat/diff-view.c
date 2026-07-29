#include "diff-view.h"

#include "chat/diff-text.h"
#include "util/unified-diff.h"

#define INLINE_EAGER_ROWS 60
#define INLINE_PREVIEW_ROWS 120
#define VIRTUAL_DIFF_ROWS 80

typedef struct
{
  GPtrArray *lines;
  GHashTable *collapsed;
} VirtualDiff;

static void
virtual_diff_free (VirtualDiff *diff)
{
  g_clear_pointer (&diff->lines, g_ptr_array_unref);
  g_clear_pointer (&diff->collapsed, g_hash_table_unref);
  g_free (diff);
}

static void
clear_box (GtkBox *box)
{
  GtkWidget *child;

  while ((child = gtk_widget_get_first_child (GTK_WIDGET (box))) != NULL)
    gtk_box_remove (box, child);
}

static void
fill_parsed_rows (GtkBox    *box,
                  GPtrArray *lines,
                  gboolean   show_file_headers,
                  guint      limit)
{
  g_autofree char *markup = NULL;
  g_autoptr (GArray) kinds = NULL;
  GtkWidget *text = xd_diff_text_new ();

  clear_box (box);
  markup = xd_unified_diff_markup (
    lines, show_file_headers, limit, &kinds);
  xd_diff_text_set_rows (XD_DIFF_TEXT (text), markup, kinds);
  gtk_widget_set_hexpand (text, TRUE);
  gtk_box_append (box, text);
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

static gboolean
file_range (GtkExpander *expander,
            guint       *start,
            guint       *end)
{
  guint first =
    GPOINTER_TO_UINT (g_object_get_data (G_OBJECT (expander), "xd-start"));
  guint last =
    GPOINTER_TO_UINT (g_object_get_data (G_OBJECT (expander), "xd-end"));

  if (first == 0 || last == 0 || first > last)
    return FALSE;

  *start = first - 1;
  *end = last - 1;
  return TRUE;
}

static void
fill_virtual_file (GtkExpander *expander,
                   VirtualDiff *diff)
{
  GtkBox *body = GTK_BOX (gtk_expander_get_child (expander));
  guint start;
  guint end;

  clear_box (body);

  if (!file_range (expander, &start, &end) || start >= diff->lines->len)
    return;

  if (((XdDiffLine *) g_ptr_array_index (diff->lines, start))->kind ==
      XD_DIFF_LINE_FILE)
    start++;

  end = MIN (end, diff->lines->len);
  for (guint at = start; at < end; at += VIRTUAL_DIFF_ROWS)
    {
      guint chunk_end = MIN (at + VIRTUAL_DIFF_ROWS, end);
      g_autoptr (GArray) kinds = NULL;
      g_autofree char *markup = xd_unified_diff_markup_slice (
        diff->lines, FALSE, at, chunk_end, &kinds);
      GtkWidget *text = xd_diff_text_new ();

      xd_diff_text_set_rows (XD_DIFF_TEXT (text), markup, kinds);
      gtk_widget_set_hexpand (text, TRUE);
      gtk_widget_add_css_class (
        GTK_WIDGET (xd_diff_text_get_label (XD_DIFF_TEXT (text))),
        "xd-diff-chunk");
      gtk_box_append (body, text);
    }
}

static void
on_virtual_file_expanded (GtkExpander *expander,
                          GParamSpec  *pspec,
                          gpointer     user_data)
{
  VirtualDiff *diff = user_data;
  const char *path =
    g_object_get_data (G_OBJECT (expander), "xd-file-path");
  GtkBox *body = GTK_BOX (gtk_expander_get_child (expander));

  if (path == NULL)
    return;

  if (gtk_expander_get_expanded (expander))
    {
      g_hash_table_remove (diff->collapsed, path);
      fill_virtual_file (expander, diff);
    }
  else
    {
      g_hash_table_add (diff->collapsed, g_strdup (path));
      clear_box (body);
    }
}

static void
setup_virtual_file (GtkSignalListItemFactory *factory,
                    GtkListItem              *item,
                    gpointer                  user_data)
{
  GtkWidget *expander = gtk_expander_new (NULL);
  GtkWidget *header = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *path = gtk_label_new (NULL);
  GtkWidget *counts = gtk_label_new (NULL);
  GtkWidget *body = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  gulong handler;

  gtk_label_set_xalign (GTK_LABEL (path), 0.0f);
  gtk_label_set_ellipsize (GTK_LABEL (path), PANGO_ELLIPSIZE_MIDDLE);
  gtk_widget_set_hexpand (path, TRUE);
  gtk_label_set_xalign (GTK_LABEL (counts), 1.0f);
  gtk_widget_set_hexpand (header, TRUE);

  gtk_box_append (GTK_BOX (header), path);
  gtk_box_append (GTK_BOX (header), counts);
  gtk_expander_set_label_widget (GTK_EXPANDER (expander), header);
  gtk_expander_set_child (GTK_EXPANDER (expander), body);
  gtk_widget_set_hexpand (expander, TRUE);
  gtk_widget_add_css_class (expander, "xd-diff-expander");

  g_object_set_data (G_OBJECT (expander), "xd-path-label", path);
  g_object_set_data (G_OBJECT (expander), "xd-counts-label", counts);
  handler = g_signal_connect (
    expander, "notify::expanded",
    G_CALLBACK (on_virtual_file_expanded), user_data);
  g_object_set_data (G_OBJECT (expander), "xd-expanded-handler",
                     GSIZE_TO_POINTER (handler));

  gtk_list_item_set_child (item, expander);
}

static void
bind_virtual_file (GtkSignalListItemFactory *factory,
                   GtkListItem              *item,
                   gpointer                  user_data)
{
  VirtualDiff *diff = user_data;
  GtkExpander *expander =
    GTK_EXPANDER (gtk_list_item_get_child (item));
  GtkLabel *path_label =
    g_object_get_data (G_OBJECT (expander), "xd-path-label");
  GtkLabel *counts_label =
    g_object_get_data (G_OBJECT (expander), "xd-counts-label");
  GtkStringObject *descriptor =
    GTK_STRING_OBJECT (gtk_list_item_get_item (item));
  const char *value = gtk_string_object_get_string (descriptor);
  char *at = NULL;
  guint64 parsed_start = g_ascii_strtoull (value, &at, 10);
  guint64 parsed_end = at != NULL && *at == ':'
    ? g_ascii_strtoull (at + 1, NULL, 10) : parsed_start;
  guint start = (guint) MIN (parsed_start, G_MAXUINT);
  guint end = (guint) MIN (parsed_end, G_MAXUINT);
  const char *path = "Changes";
  g_autofree char *path_markup = NULL;
  g_autofree char *counts_markup = NULL;
  gulong handler = GPOINTER_TO_SIZE (
    g_object_get_data (G_OBJECT (expander), "xd-expanded-handler"));
  guint additions = 0;
  guint deletions = 0;
  gboolean expanded;

  if (start < diff->lines->len)
    {
      XdDiffLine *first = g_ptr_array_index (diff->lines, start);

      if (first->kind == XD_DIFF_LINE_FILE)
        path = first->text;
    }
  end = MIN (end, diff->lines->len);

  for (guint i = start; i < end; i++)
    {
      XdDiffLine *line = g_ptr_array_index (diff->lines, i);

      if (line->kind == XD_DIFF_LINE_ADDED)
        additions++;
      else if (line->kind == XD_DIFF_LINE_REMOVED)
        deletions++;
    }

  {
    g_autofree char *valid = g_utf8_make_valid (path, -1);
    g_autofree char *escaped = g_markup_escape_text (valid, -1);

    path_markup = g_strdup_printf (
      "<span foreground=\"#ffbe6f\" weight=\"bold\">%s</span>", escaped);
  }
  counts_markup = g_strdup_printf (
    "<span foreground=\"#57e389\">+%u</span>"
    "  <span foreground=\"#f66151\">−%u</span>",
    additions, deletions);
  gtk_label_set_markup (path_label, path_markup);
  gtk_label_set_markup (counts_label, counts_markup);

  g_object_set_data_full (G_OBJECT (expander), "xd-file-path",
                          g_strdup (path), g_free);
  g_object_set_data (G_OBJECT (expander), "xd-start",
                     GUINT_TO_POINTER (start + 1));
  g_object_set_data (G_OBJECT (expander), "xd-end",
                     GUINT_TO_POINTER (end + 1));

  expanded = !g_hash_table_contains (diff->collapsed, path);
  g_signal_handler_block (expander, handler);
  gtk_expander_set_expanded (expander, expanded);
  g_signal_handler_unblock (expander, handler);

  if (expanded)
    fill_virtual_file (expander, diff);
}

static void
unbind_virtual_file (GtkSignalListItemFactory *factory,
                     GtkListItem              *item,
                     gpointer                  user_data)
{
  GtkExpander *expander =
    GTK_EXPANDER (gtk_list_item_get_child (item));
  GtkBox *body = GTK_BOX (gtk_expander_get_child (expander));

  clear_box (body);
  gtk_label_set_label (
    GTK_LABEL (g_object_get_data (G_OBJECT (expander), "xd-path-label")), "");
  gtk_label_set_label (
    GTK_LABEL (g_object_get_data (G_OBJECT (expander), "xd-counts-label")), "");
  g_object_set_data (G_OBJECT (expander), "xd-start", NULL);
  g_object_set_data (G_OBJECT (expander), "xd-end", NULL);
  g_object_set_data (G_OBJECT (expander), "xd-file-path", NULL);
}

GtkWidget *
xd_diff_view_new_file_sections (void)
{
  g_autoptr (GtkListItemFactory) factory =
    gtk_signal_list_item_factory_new ();
  GtkWidget *view = gtk_list_view_new (NULL, NULL);
  VirtualDiff *diff = g_new0 (VirtualDiff, 1);

  diff->lines =
    g_ptr_array_new_with_free_func ((GDestroyNotify) xd_diff_line_free);
  diff->collapsed =
    g_hash_table_new_full (g_str_hash, g_str_equal, g_free, NULL);
  g_object_set_data_full (
    G_OBJECT (factory), "xd-virtual-diff", diff,
    (GDestroyNotify) virtual_diff_free);
  g_signal_connect (factory, "setup",
                    G_CALLBACK (setup_virtual_file), diff);
  g_signal_connect (factory, "bind",
                    G_CALLBACK (bind_virtual_file), diff);
  g_signal_connect (factory, "unbind",
                    G_CALLBACK (unbind_virtual_file), diff);

  gtk_list_view_set_factory (
    GTK_LIST_VIEW (view), GTK_LIST_ITEM_FACTORY (factory));
  gtk_widget_add_css_class (view, "xd-diff-list");

  return view;
}

void
xd_diff_view_fill_file_sections (GtkListView *view,
                                 const char  *patch,
                                 guint       *additions,
                                 guint       *deletions)
{
  GtkListItemFactory *factory;
  VirtualDiff *diff;
  g_autoptr (GtkStringList) chunks = gtk_string_list_new (NULL);
  g_autoptr (GtkNoSelection) selection = NULL;
  guint start = 0;
  gboolean have_file = FALSE;

  g_return_if_fail (GTK_IS_LIST_VIEW (view));

  factory = gtk_list_view_get_factory (view);
  diff = g_object_get_data (G_OBJECT (factory), "xd-virtual-diff");

  /* Unbind old rows before replacing the parsed data they reference. */
  gtk_list_view_set_model (view, NULL);
  g_clear_pointer (&diff->lines, g_ptr_array_unref);
  diff->lines = xd_unified_diff_parse (patch, additions, deletions);

  for (guint i = 0; i < diff->lines->len; i++)
    {
      XdDiffLine *line = g_ptr_array_index (diff->lines, i);

      if (line->kind != XD_DIFF_LINE_FILE)
        continue;

      if (have_file)
        {
          g_autofree char *descriptor =
            g_strdup_printf ("%u:%u", start, i);

          gtk_string_list_append (chunks, descriptor);
        }

      start = i;
      have_file = TRUE;
    }

  if (have_file)
    {
      g_autofree char *descriptor =
        g_strdup_printf ("%u:%u", start, diff->lines->len);

      gtk_string_list_append (chunks, descriptor);
    }
  else if (diff->lines->len > 0)
    {
      g_autofree char *descriptor =
        g_strdup_printf ("0:%u", diff->lines->len);

      gtk_string_list_append (chunks, descriptor);
    }

  selection = gtk_no_selection_new (
    G_LIST_MODEL (g_object_ref (chunks)));
  gtk_list_view_set_model (view, GTK_SELECTION_MODEL (selection));
}
