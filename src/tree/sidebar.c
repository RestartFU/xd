#include "sidebar.h"

struct _HySidebar
{
  AdwBin parent_instance;

  HyFsTree *tree;
  GSettings *settings;
  GtkTreeListModel *tree_model;
  GtkSingleSelection *selection;
  GtkListView *list_view;

  GHashTable *expanded;     /* folder ids the user left open */
  guint save_expanded_id;
};

enum
{
  SIGNAL_NODE_SELECTED,
  SIGNAL_NODE_ACTIVATED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (HySidebar, hy_sidebar, ADW_TYPE_BIN)

/* --- small dialog helpers ------------------------------------------------- */

static void
show_error (HySidebar  *self,
            const char *heading,
            GError     *error)
{
  AdwAlertDialog *dialog;

  dialog = ADW_ALERT_DIALOG (adw_alert_dialog_new (heading, error->message));
  adw_alert_dialog_add_response (dialog, "close", "Close");
  adw_alert_dialog_set_default_response (dialog, "close");
  adw_dialog_present (ADW_DIALOG (dialog), GTK_WIDGET (self));
}

typedef void (*NameCallback) (HySidebar  *self,
                              HyNode     *node,
                              const char *name);

typedef struct
{
  HySidebar *self;
  HyNode *node;         /* unowned; owned by the tree */
  NameCallback callback;
} NamePrompt;

static void
on_name_response (GObject      *source,
                  GAsyncResult *result,
                  gpointer      data)
{
  NamePrompt *prompt = data;
  AdwAlertDialog *dialog = ADW_ALERT_DIALOG (source);
  const char *response;
  GtkEditable *entry;
  const char *name;

  response = adw_alert_dialog_choose_finish (dialog, result);
  entry = GTK_EDITABLE (adw_alert_dialog_get_extra_child (dialog));
  name = gtk_editable_get_text (entry);

  if (g_strcmp0 (response, "confirm") == 0 && *name != '\0')
    prompt->callback (prompt->self, prompt->node, name);

  g_object_unref (prompt->self);
  g_free (prompt);
}

static void
prompt_for_name (HySidebar    *self,
                 const char   *heading,
                 const char   *body,
                 const char   *confirm_label,
                 const char   *initial,
                 HyNode       *node,
                 NameCallback  callback)
{
  AdwAlertDialog *dialog;
  NamePrompt *prompt;
  GtkWidget *entry;

  dialog = ADW_ALERT_DIALOG (adw_alert_dialog_new (heading, body));
  adw_alert_dialog_add_responses (dialog,
                                  "cancel", "Cancel",
                                  "confirm", confirm_label,
                                  NULL);
  adw_alert_dialog_set_response_appearance (dialog, "confirm",
                                            ADW_RESPONSE_SUGGESTED);
  adw_alert_dialog_set_default_response (dialog, "confirm");
  adw_alert_dialog_set_close_response (dialog, "cancel");

  entry = gtk_entry_new ();
  gtk_editable_set_text (GTK_EDITABLE (entry), initial != NULL ? initial : "");
  gtk_entry_set_activates_default (GTK_ENTRY (entry), TRUE);
  adw_alert_dialog_set_extra_child (dialog, entry);

  prompt = g_new0 (NamePrompt, 1);
  prompt->self = g_object_ref (self);
  prompt->node = node;
  prompt->callback = callback;

  adw_alert_dialog_choose (dialog, GTK_WIDGET (self), NULL,
                           on_name_response, prompt);
}

/* --- actions -------------------------------------------------------------- */

/* Menu items carry the folder path, which is the only stable handle a GVariant
 * can hold; the node itself is looked up from it. */
static HyNode *
node_from_target (HySidebar *self,
                  GVariant  *target)
{
  const char *path;

  if (target == NULL)
    return NULL;

  path = g_variant_get_string (target, NULL);

  return hy_fs_tree_lookup (self->tree, path);
}

static void
create_folder (HySidebar  *self,
               HyNode     *parent,
               const char *name)
{
  g_autoptr (GError) error = NULL;

  if (hy_fs_tree_create_folder (self->tree, parent, name, &error) == NULL)
    show_error (self, "Could not create the folder", error);
}

static void
on_new_workspace (GtkWidget  *widget,
                  const char *action_name,
                  GVariant   *target)
{
  HySidebar *self = HY_SIDEBAR (widget);

  prompt_for_name (self, "New Workspace",
                   "A workspace groups the folders and chats for one company, "
                   "client or project.",
                   "Create", NULL, NULL, create_folder);
}

static void
on_new_folder (GtkWidget  *widget,
               const char *action_name,
               GVariant   *target)
{
  HySidebar *self = HY_SIDEBAR (widget);
  HyNode *parent = node_from_target (self, target);

  if (parent == NULL)
    return;

  prompt_for_name (self, "New Folder", NULL, "Create", NULL,
                   parent, create_folder);
}

static void
rename_folder (HySidebar  *self,
               HyNode     *node,
               const char *name)
{
  g_autoptr (GError) error = NULL;

  if (!hy_fs_tree_rename_folder (self->tree, node, name, &error))
    show_error (self, "Could not rename the folder", error);
}

static void
on_rename (GtkWidget  *widget,
           const char *action_name,
           GVariant   *target)
{
  HySidebar *self = HY_SIDEBAR (widget);
  HyNode *node = node_from_target (self, target);

  if (node == NULL)
    return;

  prompt_for_name (self, "Rename Folder", NULL, "Rename",
                   hy_node_get_name (node), node, rename_folder);
}

/* --- chats ---------------------------------------------------------------- */

static HyNode *
chat_from_target (HySidebar *self,
                  GVariant  *target)
{
  if (target == NULL)
    return NULL;

  return hy_fs_tree_lookup_chat (self->tree, g_variant_get_string (target, NULL));
}

/*
 * New chats ask for a working directory, because a folder is an
 * organisational thing: "Lunar / Proxy" may want the proxy repo one day and a
 * scratch checkout the next, and neither of them lives inside the workspace
 * tree. Leaving it unset inherits the folder's own directory.
 */
typedef struct
{
  HySidebar *self;
  HyNode *folder;           /* unowned; owned by the tree */
  GtkEditable *title_entry;
  GtkButton *dir_button;
  char *workdir;            /* NULL: inherit */
} NewChatPrompt;

static void
new_chat_prompt_free (NewChatPrompt *prompt)
{
  g_object_unref (prompt->self);
  g_free (prompt->workdir);
  g_free (prompt);
}

static void
update_dir_button (NewChatPrompt *prompt)
{
  if (prompt->workdir != NULL)
    {
      g_autofree char *name = g_path_get_basename (prompt->workdir);

      gtk_button_set_label (prompt->dir_button, name);
      gtk_widget_set_tooltip_text (GTK_WIDGET (prompt->dir_button), prompt->workdir);
    }
  else
    {
      gtk_button_set_label (prompt->dir_button, "Same as folder");
      gtk_widget_set_tooltip_text (GTK_WIDGET (prompt->dir_button),
                                   hy_node_get_path (prompt->folder));
    }
}

static void
on_directory_chosen (GObject      *source,
                     GAsyncResult *result,
                     gpointer      user_data)
{
  NewChatPrompt *prompt = user_data;
  g_autoptr (GFile) folder = NULL;

  folder = gtk_file_dialog_select_folder_finish (GTK_FILE_DIALOG (source),
                                                 result, NULL);
  if (folder == NULL)
    return;

  g_free (prompt->workdir);
  prompt->workdir = g_file_get_path (folder);

  update_dir_button (prompt);
}

static void
on_choose_directory (GtkButton *button,
                     gpointer   user_data)
{
  NewChatPrompt *prompt = user_data;
  g_autoptr (GtkFileDialog) dialog = gtk_file_dialog_new ();
  g_autofree char *projects_root = NULL;
  GtkRoot *root = gtk_widget_get_root (GTK_WIDGET (prompt->self));

  gtk_file_dialog_set_title (dialog, "Working Directory");

  /* Start where the user's repositories actually live, not in the workspace
   * tree, which holds no code. */
  projects_root = g_settings_get_string (prompt->self->settings, "projects-root");
  if (projects_root == NULL || *projects_root == '\0')
    {
      g_free (projects_root);
      projects_root = g_build_filename (g_get_home_dir (), "projects", NULL);
    }

  if (g_file_test (projects_root, G_FILE_TEST_IS_DIR))
    {
      g_autoptr (GFile) initial = g_file_new_for_path (projects_root);

      gtk_file_dialog_set_initial_folder (dialog, initial);
    }

  gtk_file_dialog_select_folder (dialog, GTK_WINDOW (root), NULL,
                                 on_directory_chosen, prompt);
}

static void
on_new_chat_response (GObject      *source,
                      GAsyncResult *result,
                      gpointer      user_data)
{
  NewChatPrompt *prompt = user_data;
  g_autoptr (GError) error = NULL;
  g_autofree char *backend = NULL;
  const char *response;
  const char *title;
  HyNode *chat;

  response = adw_alert_dialog_choose_finish (ADW_ALERT_DIALOG (source), result);

  if (g_strcmp0 (response, "confirm") != 0)
    {
      new_chat_prompt_free (prompt);
      return;
    }

  title = gtk_editable_get_text (prompt->title_entry);
  if (*title == '\0')
    title = "New Chat";

  backend = g_settings_get_string (prompt->self->settings, "default-backend");

  chat = hy_fs_tree_create_chat (prompt->self->tree, prompt->folder, title,
                                 backend, prompt->workdir, &error);
  if (chat == NULL)
    show_error (prompt->self, "Could not start the chat", error);
  else
    g_signal_emit (prompt->self, signals[SIGNAL_NODE_ACTIVATED], 0, chat);

  new_chat_prompt_free (prompt);
}

static void
on_new_chat (GtkWidget  *widget,
             const char *action_name,
             GVariant   *target)
{
  HySidebar *self = HY_SIDEBAR (widget);
  HyNode *folder = node_from_target (self, target);
  NewChatPrompt *prompt;
  AdwAlertDialog *dialog;
  GtkWidget *box;
  GtkWidget *dir_row;
  GtkWidget *dir_label;

  if (folder == NULL)
    return;

  prompt = g_new0 (NewChatPrompt, 1);
  prompt->self = g_object_ref (self);
  prompt->folder = folder;

  dialog = ADW_ALERT_DIALOG (adw_alert_dialog_new ("New Chat", NULL));
  adw_alert_dialog_add_responses (dialog,
                                  "cancel", "Cancel",
                                  "confirm", "Create",
                                  NULL);
  adw_alert_dialog_set_response_appearance (dialog, "confirm",
                                            ADW_RESPONSE_SUGGESTED);
  adw_alert_dialog_set_default_response (dialog, "confirm");
  adw_alert_dialog_set_close_response (dialog, "cancel");

  box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 12);

  prompt->title_entry = GTK_EDITABLE (gtk_entry_new ());
  gtk_entry_set_placeholder_text (GTK_ENTRY (prompt->title_entry), "Chat name");
  gtk_entry_set_activates_default (GTK_ENTRY (prompt->title_entry), TRUE);
  gtk_box_append (GTK_BOX (box), GTK_WIDGET (prompt->title_entry));

  dir_row = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  dir_label = gtk_label_new ("Runs in");
  gtk_widget_add_css_class (dir_label, "dim-label");

  prompt->dir_button = GTK_BUTTON (gtk_button_new ());
  gtk_widget_set_hexpand (GTK_WIDGET (prompt->dir_button), TRUE);
  g_signal_connect (prompt->dir_button, "clicked",
                    G_CALLBACK (on_choose_directory), prompt);
  update_dir_button (prompt);

  gtk_box_append (GTK_BOX (dir_row), dir_label);
  gtk_box_append (GTK_BOX (dir_row), GTK_WIDGET (prompt->dir_button));
  gtk_box_append (GTK_BOX (box), dir_row);

  adw_alert_dialog_set_extra_child (dialog, box);

  adw_alert_dialog_choose (dialog, GTK_WIDGET (self), NULL,
                           on_new_chat_response, prompt);
}

static void
rename_chat (HySidebar  *self,
             HyNode     *chat,
             const char *title)
{
  g_autoptr (GError) error = NULL;

  if (!hy_fs_tree_rename_chat (self->tree, chat, title, &error))
    show_error (self, "Could not rename the chat", error);
}

static void
on_rename_chat (GtkWidget  *widget,
                const char *action_name,
                GVariant   *target)
{
  HySidebar *self = HY_SIDEBAR (widget);
  HyNode *chat = chat_from_target (self, target);

  if (chat == NULL)
    return;

  prompt_for_name (self, "Rename Chat", NULL, "Rename",
                   hy_node_get_name (chat), chat, rename_chat);
}

static void
on_delete_chat (GtkWidget  *widget,
                const char *action_name,
                GVariant   *target)
{
  HySidebar *self = HY_SIDEBAR (widget);
  HyNode *chat = chat_from_target (self, target);
  g_autoptr (GError) error = NULL;

  if (chat == NULL)
    return;

  if (!hy_fs_tree_delete_chat (self->tree, chat, &error))
    show_error (self, "Could not delete the chat", error);
}

typedef struct
{
  HySidebar *self;
  HyNode *node;
} TrashPrompt;

static void
on_trash_response (GObject      *source,
                   GAsyncResult *result,
                   gpointer      data)
{
  TrashPrompt *prompt = data;
  g_autoptr (GError) error = NULL;
  const char *response;

  response = adw_alert_dialog_choose_finish (ADW_ALERT_DIALOG (source), result);

  if (g_strcmp0 (response, "trash") == 0 &&
      !hy_fs_tree_trash_folder (prompt->self->tree, prompt->node, &error))
    show_error (prompt->self, "Could not move the folder to the trash", error);

  g_object_unref (prompt->self);
  g_free (prompt);
}

static void
on_trash (GtkWidget  *widget,
          const char *action_name,
          GVariant   *target)
{
  HySidebar *self = HY_SIDEBAR (widget);
  HyNode *node = node_from_target (self, target);
  g_autofree char *body = NULL;
  AdwAlertDialog *dialog;
  TrashPrompt *prompt;

  if (node == NULL)
    return;

  body = g_strdup_printf ("“%s” and everything inside it will be moved to the "
                          "trash.", hy_node_get_name (node));

  dialog = ADW_ALERT_DIALOG (adw_alert_dialog_new ("Move Folder to Trash?", body));
  adw_alert_dialog_add_responses (dialog,
                                  "cancel", "Cancel",
                                  "trash", "Move to Trash",
                                  NULL);
  adw_alert_dialog_set_response_appearance (dialog, "trash",
                                            ADW_RESPONSE_DESTRUCTIVE);
  adw_alert_dialog_set_close_response (dialog, "cancel");

  prompt = g_new0 (TrashPrompt, 1);
  prompt->self = g_object_ref (self);
  prompt->node = node;

  adw_alert_dialog_choose (dialog, GTK_WIDGET (self), NULL,
                           on_trash_response, prompt);
}

/* --- list view ------------------------------------------------------------ */

static GListModel *
create_child_model (gpointer item,
                    gpointer user_data)
{
  HyNode *node = item;

  if (hy_node_get_kind (node) != HY_NODE_FOLDER)
    return NULL;

  return G_LIST_MODEL (g_object_ref (hy_node_get_children (node)));
}

/* --- expansion state ------------------------------------------------------ */

/*
 * Which folders are open is remembered by folder id, not by path, so the tree
 * comes back the way it was left even if a folder was renamed or moved in
 * between. Rows are bound as their parents expand, so restoring happens
 * naturally from the root down.
 */

static gboolean
save_expanded (gpointer user_data)
{
  HySidebar *self = user_data;
  g_autoptr (GPtrArray) ids = g_ptr_array_new ();
  GHashTableIter iter;
  gpointer id;

  self->save_expanded_id = 0;

  g_hash_table_iter_init (&iter, self->expanded);
  while (g_hash_table_iter_next (&iter, &id, NULL))
    g_ptr_array_add (ids, id);
  g_ptr_array_add (ids, NULL);

  g_settings_set_strv (self->settings, "expanded-folders",
                       (const char * const *) ids->pdata);

  return G_SOURCE_REMOVE;
}

/* Expanding a deep branch toggles many rows at once; coalesce the writes. */
static void
queue_save_expanded (HySidebar *self)
{
  if (self->save_expanded_id == 0)
    self->save_expanded_id = g_idle_add (save_expanded, self);
}

static void
on_row_expanded (GtkTreeListRow *row,
                 GParamSpec     *pspec,
                 gpointer        user_data)
{
  HySidebar *self = user_data;
  g_autoptr (HyNode) node = gtk_tree_list_row_get_item (row);
  const char *folder_id;

  if (node == NULL || hy_node_get_kind (node) != HY_NODE_FOLDER)
    return;

  folder_id = hy_node_get_folder_id (node);
  if (folder_id == NULL)
    return;

  if (gtk_tree_list_row_get_expanded (row))
    g_hash_table_add (self->expanded, g_strdup (folder_id));
  else
    g_hash_table_remove (self->expanded, folder_id);

  queue_save_expanded (self);
}

static void
on_item_setup (GtkSignalListItemFactory *factory,
               GtkListItem              *item,
               gpointer                  user_data)
{
  GtkWidget *expander = gtk_tree_expander_new ();
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *icon = gtk_image_new ();
  GtkWidget *label = gtk_label_new (NULL);
  GtkWidget *menu_button = gtk_menu_button_new ();

  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_label_set_ellipsize (GTK_LABEL (label), PANGO_ELLIPSIZE_END);
  gtk_widget_set_hexpand (label, TRUE);

  gtk_menu_button_set_icon_name (GTK_MENU_BUTTON (menu_button), "view-more-symbolic");
  gtk_widget_add_css_class (menu_button, "flat");
  gtk_widget_set_valign (menu_button, GTK_ALIGN_CENTER);

  gtk_box_append (GTK_BOX (box), icon);
  gtk_box_append (GTK_BOX (box), label);
  gtk_box_append (GTK_BOX (box), menu_button);

  gtk_tree_expander_set_child (GTK_TREE_EXPANDER (expander), box);
  gtk_list_item_set_child (item, expander);
}

static GMenuModel *
build_row_menu (HyNode *node)
{
  g_autoptr (GVariant) target =
    g_variant_ref_sink (g_variant_new_string (hy_node_get_path (node)));
  GMenu *menu = g_menu_new ();
  GMenu *section = g_menu_new ();
  g_autoptr (GMenuItem) new_chat = NULL;
  g_autoptr (GMenuItem) new_folder = NULL;
  g_autoptr (GMenuItem) rename = NULL;
  g_autoptr (GMenuItem) trash = NULL;

  new_chat = g_menu_item_new ("New Chat", NULL);
  g_menu_item_set_action_and_target_value (new_chat, "sidebar.new-chat", target);
  g_menu_append_item (menu, new_chat);

  new_folder = g_menu_item_new ("New Folder", NULL);
  g_menu_item_set_action_and_target_value (new_folder, "sidebar.new-folder", target);
  g_menu_append_item (menu, new_folder);

  rename = g_menu_item_new ("Rename…", NULL);
  g_menu_item_set_action_and_target_value (rename, "sidebar.rename", target);
  g_menu_append_item (menu, rename);

  trash = g_menu_item_new ("Move to Trash", NULL);
  g_menu_item_set_action_and_target_value (trash, "sidebar.trash", target);
  g_menu_append_item (section, trash);
  g_menu_append_section (menu, NULL, G_MENU_MODEL (section));
  g_object_unref (section);

  return G_MENU_MODEL (menu);
}

static GMenuModel *
build_chat_menu (HyNode *node)
{
  g_autoptr (GVariant) target =
    g_variant_ref_sink (g_variant_new_string (hy_node_get_chat_id (node)));
  GMenu *menu = g_menu_new ();
  GMenu *section = g_menu_new ();
  g_autoptr (GMenuItem) rename = NULL;
  g_autoptr (GMenuItem) delete = NULL;

  rename = g_menu_item_new ("Rename…", NULL);
  g_menu_item_set_action_and_target_value (rename, "sidebar.rename-chat", target);
  g_menu_append_item (menu, rename);

  delete = g_menu_item_new ("Delete Chat", NULL);
  g_menu_item_set_action_and_target_value (delete, "sidebar.delete-chat", target);
  g_menu_append_item (section, delete);
  g_menu_append_section (menu, NULL, G_MENU_MODEL (section));
  g_object_unref (section);

  return G_MENU_MODEL (menu);
}

static void
on_item_bind (GtkSignalListItemFactory *factory,
              GtkListItem              *item,
              gpointer                  user_data)
{
  HySidebar *self = user_data;
  GtkTreeListRow *row = gtk_list_item_get_item (item);
  GtkWidget *expander = gtk_list_item_get_child (item);
  GtkWidget *box = gtk_tree_expander_get_child (GTK_TREE_EXPANDER (expander));
  GtkWidget *icon = gtk_widget_get_first_child (box);
  GtkWidget *label = gtk_widget_get_next_sibling (icon);
  GtkWidget *menu_button = gtk_widget_get_next_sibling (label);
  g_autoptr (HyNode) node = gtk_tree_list_row_get_item (row);

  gtk_tree_expander_set_list_row (GTK_TREE_EXPANDER (expander), row);
  gtk_image_set_from_icon_name (GTK_IMAGE (icon), hy_node_get_icon_name (node));

  g_object_set_data (G_OBJECT (item), "name-binding",
                     g_object_bind_property (node, "name", label, "label",
                                             G_BINDING_SYNC_CREATE));

  {
    g_autoptr (GMenuModel) menu = hy_node_get_kind (node) == HY_NODE_FOLDER
                                    ? build_row_menu (node)
                                    : build_chat_menu (node);

    gtk_menu_button_set_menu_model (GTK_MENU_BUTTON (menu_button), menu);
  }

  if (hy_node_get_kind (node) == HY_NODE_FOLDER)
    {
      const char *folder_id = hy_node_get_folder_id (node);
      gulong handler;

      /* Restore before listening, or restoring would itself be recorded. */
      if (folder_id != NULL)
        gtk_tree_list_row_set_expanded (row,
                                        g_hash_table_contains (self->expanded,
                                                               folder_id));

      handler = g_signal_connect (row, "notify::expanded",
                                  G_CALLBACK (on_row_expanded), self);
      g_object_set_data (G_OBJECT (item), "expanded-handler",
                         GSIZE_TO_POINTER (handler));
    }
}

static void
on_item_unbind (GtkSignalListItemFactory *factory,
                GtkListItem              *item,
                gpointer                  user_data)
{
  GBinding *binding = g_object_get_data (G_OBJECT (item), "name-binding");
  gpointer handler = g_object_get_data (G_OBJECT (item), "expanded-handler");
  GtkTreeListRow *row = gtk_list_item_get_item (item);

  /* Rows are recycled, so the label must stop following the node it showed
   * before, or it keeps updating on behalf of a row it no longer represents. */
  if (binding != NULL)
    {
      g_binding_unbind (binding);
      g_object_set_data (G_OBJECT (item), "name-binding", NULL);
    }

  if (handler != NULL && row != NULL)
    {
      g_signal_handler_disconnect (row, GPOINTER_TO_SIZE (handler));
      g_object_set_data (G_OBJECT (item), "expanded-handler", NULL);
    }
}

static void
on_selection_changed (GtkSingleSelection *selection,
                      GParamSpec         *pspec,
                      gpointer            user_data)
{
  HySidebar *self = user_data;
  GtkTreeListRow *row = gtk_single_selection_get_selected_item (selection);
  g_autoptr (HyNode) node = NULL;

  if (row != NULL)
    node = gtk_tree_list_row_get_item (row);

  g_signal_emit (self, signals[SIGNAL_NODE_SELECTED], 0, node);
}

static void
on_row_activated (GtkListView *list_view,
                  guint        position,
                  gpointer     user_data)
{
  HySidebar *self = user_data;
  g_autoptr (GtkTreeListRow) row = NULL;
  g_autoptr (HyNode) node = NULL;

  row = g_list_model_get_item (G_LIST_MODEL (self->selection), position);
  if (row == NULL)
    return;

  node = gtk_tree_list_row_get_item (row);

  /* Double-clicking a folder is the natural "open/close" gesture. */
  if (hy_node_get_kind (node) == HY_NODE_FOLDER)
    gtk_tree_list_row_set_expanded (row, !gtk_tree_list_row_get_expanded (row));
  else
    g_signal_emit (self, signals[SIGNAL_NODE_ACTIVATED], 0, node);
}

/* --- construction --------------------------------------------------------- */

HySidebar *
hy_sidebar_new (HyFsTree *tree)
{
  HySidebar *self;

  g_return_val_if_fail (HY_IS_FS_TREE (tree), NULL);

  self = g_object_new (HY_TYPE_SIDEBAR, NULL);
  self->tree = g_object_ref (tree);

  self->tree_model = gtk_tree_list_model_new (g_object_ref (hy_fs_tree_get_model (tree)),
                                              FALSE, FALSE,
                                              create_child_model, NULL, NULL);
  self->selection = gtk_single_selection_new (g_object_ref (G_LIST_MODEL (self->tree_model)));
  gtk_single_selection_set_autoselect (self->selection, FALSE);
  gtk_single_selection_set_can_unselect (self->selection, TRUE);

  gtk_list_view_set_model (self->list_view, GTK_SELECTION_MODEL (self->selection));

  g_signal_connect (self->selection, "notify::selected",
                    G_CALLBACK (on_selection_changed), self);
  g_signal_connect (self->list_view, "activate",
                    G_CALLBACK (on_row_activated), self);

  return self;
}

static void
hy_sidebar_dispose (GObject *object)
{
  HySidebar *self = HY_SIDEBAR (object);

  if (self->save_expanded_id != 0)
    {
      g_clear_handle_id (&self->save_expanded_id, g_source_remove);
      save_expanded (self);
    }

  g_clear_pointer (&self->expanded, g_hash_table_unref);
  g_clear_object (&self->selection);
  g_clear_object (&self->tree_model);
  g_clear_object (&self->settings);
  g_clear_object (&self->tree);

  G_OBJECT_CLASS (hy_sidebar_parent_class)->dispose (object);
}

static void
hy_sidebar_class_init (HySidebarClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);
  GtkWidgetClass *widget_class = GTK_WIDGET_CLASS (klass);

  object_class->dispose = hy_sidebar_dispose;

  signals[SIGNAL_NODE_SELECTED] =
    g_signal_new ("node-selected", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, HY_TYPE_NODE);

  signals[SIGNAL_NODE_ACTIVATED] =
    g_signal_new ("node-activated", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, HY_TYPE_NODE);

  gtk_widget_class_install_action (widget_class, "sidebar.new-workspace", NULL,
                                   on_new_workspace);
  gtk_widget_class_install_action (widget_class, "sidebar.new-folder", "s",
                                   on_new_folder);
  gtk_widget_class_install_action (widget_class, "sidebar.rename", "s",
                                   on_rename);
  gtk_widget_class_install_action (widget_class, "sidebar.trash", "s",
                                   on_trash);
  gtk_widget_class_install_action (widget_class, "sidebar.new-chat", "s",
                                   on_new_chat);
  gtk_widget_class_install_action (widget_class, "sidebar.rename-chat", "s",
                                   on_rename_chat);
  gtk_widget_class_install_action (widget_class, "sidebar.delete-chat", "s",
                                   on_delete_chat);
}

static void
hy_sidebar_init (HySidebar *self)
{
  GtkWidget *toolbar = adw_toolbar_view_new ();
  GtkWidget *header = adw_header_bar_new ();
  GtkWidget *new_button = gtk_button_new_from_icon_name ("list-add-symbolic");
  GtkWidget *scrolled = gtk_scrolled_window_new ();
  GtkListItemFactory *factory = gtk_signal_list_item_factory_new ();

  g_auto (GStrv) expanded = NULL;

  self->settings = g_settings_new (HY_APP_ID);
  self->expanded = g_hash_table_new_full (g_str_hash, g_str_equal, g_free, NULL);

  expanded = g_settings_get_strv (self->settings, "expanded-folders");
  for (gsize i = 0; expanded[i] != NULL; i++)
    g_hash_table_add (self->expanded, g_strdup (expanded[i]));

  gtk_widget_set_tooltip_text (new_button, "New Workspace");
  gtk_actionable_set_action_name (GTK_ACTIONABLE (new_button), "sidebar.new-workspace");
  adw_header_bar_pack_start (ADW_HEADER_BAR (header), new_button);
  adw_toolbar_view_add_top_bar (ADW_TOOLBAR_VIEW (toolbar), header);

  g_signal_connect (factory, "setup", G_CALLBACK (on_item_setup), self);
  g_signal_connect (factory, "bind", G_CALLBACK (on_item_bind), self);
  g_signal_connect (factory, "unbind", G_CALLBACK (on_item_unbind), self);

  self->list_view = GTK_LIST_VIEW (gtk_list_view_new (NULL, factory));
  gtk_list_view_set_single_click_activate (self->list_view, FALSE);
  gtk_widget_add_css_class (GTK_WIDGET (self->list_view), "navigation-sidebar");

  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scrolled),
                                  GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (scrolled),
                                 GTK_WIDGET (self->list_view));
  gtk_widget_set_vexpand (scrolled, TRUE);

  adw_toolbar_view_set_content (ADW_TOOLBAR_VIEW (toolbar), scrolled);
  adw_bin_set_child (ADW_BIN (self), toolbar);
}
