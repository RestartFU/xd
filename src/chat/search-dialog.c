#include "search-dialog.h"

#define MAX_RESULTS 40
#define SNIPPET_LENGTH 120

typedef struct
{
  HyStorage *storage;
  HyFsTree *tree;
  HySearchActivateFunc on_activate;
  gpointer user_data;

  AdwDialog *dialog;
  GtkSearchEntry *entry;
  GtkListBox *results;
  GtkStack *stack;
  AdwStatusPage *placeholder;
} Search;

static void
search_free (gpointer data)
{
  Search *search = data;

  g_clear_object (&search->storage);
  g_clear_object (&search->tree);
  g_free (search);
}

/*
 * FTS5 reads its input as a query language, so anything the user types has to
 * be quoted before it gets there or a stray quote or hyphen is a syntax error.
 * The trailing * makes it match as you type.
 */
static char *
build_fts_query (const char *text)
{
  g_autoptr (GString) query = g_string_new (NULL);
  g_auto (GStrv) words = NULL;

  words = g_strsplit_set (text, " \t\n", -1);

  for (gsize i = 0; words[i] != NULL; i++)
    {
      g_auto (GStrv) parts = NULL;
      g_autofree char *escaped = NULL;

      if (*words[i] == '\0')
        continue;

      /* Doubling embedded quotes is how FTS5 escapes them. */
      parts = g_strsplit (words[i], "\"", -1);
      escaped = g_strjoinv ("\"\"", parts);

      if (query->len > 0)
        g_string_append_c (query, ' ');

      g_string_append_printf (query, "\"%s\"*", escaped);
    }

  if (query->len == 0)
    return NULL;

  return g_string_free (g_steal_pointer (&query), FALSE);
}

static char *
make_snippet (const char *content)
{
  g_autofree char *flattened = NULL;
  glong length;

  flattened = g_strdup (content);
  g_strdelimit (flattened, "\n\r\t", ' ');
  g_strstrip (flattened);

  length = g_utf8_strlen (flattened, -1);
  if (length <= SNIPPET_LENGTH)
    return g_steal_pointer (&flattened);

  {
    g_autofree char *shortened = g_utf8_substring (flattened, 0, SNIPPET_LENGTH);

    return g_strconcat (shortened, "…", NULL);
  }
}

static void
on_result_activated (GtkListBox    *box,
                     GtkListBoxRow *row,
                     gpointer       user_data)
{
  Search *search = user_data;
  const char *chat_id = g_object_get_data (G_OBJECT (row), "chat-id");
  HyNode *chat;

  if (chat_id == NULL)
    return;

  chat = hy_fs_tree_lookup_chat (search->tree, chat_id);
  if (chat == NULL)
    return;

  search->on_activate (chat, search->user_data);
  adw_dialog_close (search->dialog);
}

static void
clear_results (Search *search)
{
  GtkWidget *child;

  while ((child = gtk_widget_get_first_child (GTK_WIDGET (search->results))) != NULL)
    gtk_list_box_remove (search->results, child);
}

static void
show_placeholder (Search     *search,
                  const char *title,
                  const char *description)
{
  adw_status_page_set_title (search->placeholder, title);
  adw_status_page_set_description (search->placeholder, description);
  gtk_stack_set_visible_child_name (search->stack, "placeholder");
}

static void
on_search_changed (GtkSearchEntry *entry,
                   gpointer        user_data)
{
  Search *search = user_data;
  g_autoptr (GPtrArray) hits = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *query = NULL;
  const char *text = gtk_editable_get_text (GTK_EDITABLE (entry));

  clear_results (search);

  query = build_fts_query (text);
  if (query == NULL)
    {
      show_placeholder (search, "Search Chats",
                        "Find a conversation by something said in it.");
      return;
    }

  hits = hy_storage_search (search->storage, query, MAX_RESULTS, &error);
  if (hits == NULL)
    {
      show_placeholder (search, "Search Failed", error->message);
      return;
    }

  if (hits->len == 0)
    {
      show_placeholder (search, "No Results", "Nothing matches that.");
      return;
    }

  for (guint i = 0; i < hits->len; i++)
    {
      const HyMessage *message = g_ptr_array_index (hits, i);
      g_autoptr (HyChat) chat = NULL;
      g_autofree char *snippet = NULL;
      GtkWidget *row;

      chat = hy_storage_get_chat (search->storage, message->chat_id, NULL);
      if (chat == NULL)
        continue;

      snippet = make_snippet (message->content);

      row = adw_action_row_new ();
      adw_preferences_row_set_title (ADW_PREFERENCES_ROW (row),
                                     chat->title != NULL ? chat->title : "Untitled");
      adw_action_row_set_subtitle (ADW_ACTION_ROW (row), snippet);
      adw_action_row_set_subtitle_lines (ADW_ACTION_ROW (row), 2);
      gtk_list_box_row_set_activatable (GTK_LIST_BOX_ROW (row), TRUE);

      g_object_set_data_full (G_OBJECT (row), "chat-id",
                              g_strdup (message->chat_id), g_free);

      gtk_list_box_append (search->results, row);
    }

  gtk_stack_set_visible_child_name (search->stack, "results");
}

void
hy_search_dialog_present (GtkWidget            *parent,
                          HyStorage            *storage,
                          HyFsTree             *tree,
                          HySearchActivateFunc  on_activate,
                          gpointer              user_data)
{
  Search *search;
  GtkWidget *toolbar;
  GtkWidget *header;
  GtkWidget *scroller;
  GtkWidget *content;

  g_return_if_fail (HY_IS_STORAGE (storage));
  g_return_if_fail (HY_IS_FS_TREE (tree));

  search = g_new0 (Search, 1);
  search->storage = g_object_ref (storage);
  search->tree = g_object_ref (tree);
  search->on_activate = on_activate;
  search->user_data = user_data;

  search->dialog = ADW_DIALOG (adw_dialog_new ());
  adw_dialog_set_title (search->dialog, "Search");
  adw_dialog_set_content_width (search->dialog, 560);
  adw_dialog_set_content_height (search->dialog, 480);

  search->entry = GTK_SEARCH_ENTRY (gtk_search_entry_new ());
  gtk_widget_set_hexpand (GTK_WIDGET (search->entry), TRUE);
  g_signal_connect (search->entry, "search-changed",
                    G_CALLBACK (on_search_changed), search);

  header = adw_header_bar_new ();
  adw_header_bar_set_title_widget (ADW_HEADER_BAR (header), GTK_WIDGET (search->entry));

  search->results = GTK_LIST_BOX (gtk_list_box_new ());
  gtk_list_box_set_selection_mode (search->results, GTK_SELECTION_NONE);
  gtk_widget_add_css_class (GTK_WIDGET (search->results), "boxed-list");
  gtk_widget_set_valign (GTK_WIDGET (search->results), GTK_ALIGN_START);
  gtk_widget_set_margin_top (GTK_WIDGET (search->results), 12);
  gtk_widget_set_margin_bottom (GTK_WIDGET (search->results), 12);
  gtk_widget_set_margin_start (GTK_WIDGET (search->results), 12);
  gtk_widget_set_margin_end (GTK_WIDGET (search->results), 12);
  g_signal_connect (search->results, "row-activated",
                    G_CALLBACK (on_result_activated), search);

  scroller = gtk_scrolled_window_new ();
  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scroller),
                                  GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (scroller),
                                 GTK_WIDGET (search->results));

  search->placeholder = ADW_STATUS_PAGE (adw_status_page_new ());
  adw_status_page_set_icon_name (search->placeholder, "system-search-symbolic");

  search->stack = GTK_STACK (gtk_stack_new ());
  gtk_stack_add_named (search->stack, GTK_WIDGET (search->placeholder), "placeholder");
  gtk_stack_add_named (search->stack, scroller, "results");
  gtk_widget_set_vexpand (GTK_WIDGET (search->stack), TRUE);

  show_placeholder (search, "Search Chats",
                    "Find a conversation by something said in it.");

  toolbar = adw_toolbar_view_new ();
  adw_toolbar_view_add_top_bar (ADW_TOOLBAR_VIEW (toolbar), header);
  content = GTK_WIDGET (search->stack);
  adw_toolbar_view_set_content (ADW_TOOLBAR_VIEW (toolbar), content);

  adw_dialog_set_child (search->dialog, toolbar);
  g_object_set_data_full (G_OBJECT (search->dialog), "search", search, search_free);

  adw_dialog_present (search->dialog, parent);
  gtk_widget_grab_focus (GTK_WIDGET (search->entry));
}
