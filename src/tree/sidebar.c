#include "sidebar.h"

#include "backend/backend.h"
#include "settings/folder-settings-dialog.h"
#include "settings/settings-resolver.h"

struct _XdSidebar
{
  AdwBin parent_instance;

  XdFsTree *tree;
  XdRemoteTree *remote;
  GSettings *settings;
  GListStore *roots;        /* the models whose rows sit at the top level */
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

G_DEFINE_FINAL_TYPE (XdSidebar, xd_sidebar, ADW_TYPE_BIN)

/* --- small dialog helpers ------------------------------------------------- */

static void
show_error_message (XdSidebar  *self,
                    const char *heading,
                    const char *message)
{
  AdwAlertDialog *dialog;

  dialog = ADW_ALERT_DIALOG (adw_alert_dialog_new (heading, message));
  adw_alert_dialog_add_response (dialog, "close", "Close");
  adw_alert_dialog_set_default_response (dialog, "close");
  adw_dialog_present (ADW_DIALOG (dialog), GTK_WIDGET (self));
}

static void
show_error (XdSidebar  *self,
            const char *heading,
            GError     *error)
{
  show_error_message (self, heading, error->message);
}

typedef void (*NameCallback) (XdSidebar  *self,
                              XdNode     *node,
                              const char *name);

typedef struct
{
  XdSidebar *self;
  XdNode *node;         /* unowned; owned by the tree */
  GtkEditable *entry;
  NameCallback callback;
} NamePrompt;

static void
on_name_response (GObject      *source,
                  GAsyncResult *result,
                  gpointer      data)
{
  NamePrompt *prompt = data;
  const char *response;
  const char *name;

  response = adw_alert_dialog_choose_finish (ADW_ALERT_DIALOG (source), result);
  name = gtk_editable_get_text (prompt->entry);

  if (g_strcmp0 (response, "confirm") == 0 && *name != '\0')
    prompt->callback (prompt->self, prompt->node, name);

  g_object_unref (prompt->self);
  g_free (prompt);
}

static void
prompt_for_name (XdSidebar    *self,
                 const char   *heading,
                 const char   *body,
                 const char   *confirm_label,
                 const char   *initial,
                 XdNode       *node,
                 NameCallback  callback)
{
  AdwAlertDialog *dialog;
  NamePrompt *prompt;
  GtkWidget *group;
  GtkWidget *row;

  dialog = ADW_ALERT_DIALOG (adw_alert_dialog_new (heading, body));
  adw_alert_dialog_add_responses (dialog,
                                  "cancel", "Cancel",
                                  "confirm", confirm_label,
                                  NULL);
  adw_alert_dialog_set_response_appearance (dialog, "confirm",
                                            ADW_RESPONSE_SUGGESTED);
  adw_alert_dialog_set_default_response (dialog, "confirm");
  adw_alert_dialog_set_close_response (dialog, "cancel");

  /* A titled row rather than a bare entry, so the field looks like the rest
   * of the interface instead of a box dropped into a dialog. */
  row = adw_entry_row_new ();
  adw_preferences_row_set_title (ADW_PREFERENCES_ROW (row), "Name");
  gtk_editable_set_text (GTK_EDITABLE (row), initial != NULL ? initial : "");
  g_object_set (row, "activates-default", TRUE, NULL);

  group = adw_preferences_group_new ();
  adw_preferences_group_add (ADW_PREFERENCES_GROUP (group), row);
  adw_alert_dialog_set_extra_child (dialog, group);

  prompt = g_new0 (NamePrompt, 1);
  prompt->self = g_object_ref (self);
  prompt->node = node;
  prompt->entry = GTK_EDITABLE (row);
  prompt->callback = callback;

  adw_alert_dialog_choose (dialog, GTK_WIDGET (self), NULL,
                           on_name_response, prompt);
}

/* --- actions -------------------------------------------------------------- */

/*
 * True for a row that belongs to a remote rather than to this machine.
 *
 * A chat has no path of its own, so it answers for the folder holding it --
 * which is the folder the row looks like it is part of.
 */
static gboolean
is_remote_row (XdNode *node)
{
  XdNode *folder = xd_node_get_kind (node) == XD_NODE_FOLDER
    ? node : xd_node_get_parent (node);

  return folder != NULL && xd_remote_tree_is_remote_path (xd_node_get_path (folder));
}

/* Menu items carry the folder path, which is the only stable handle a GVariant
 * can hold; the node itself is looked up from it. A remote's paths are URIs,
 * so which tree to ask is written on the target. */
static XdNode *
node_from_target (XdSidebar *self,
                  GVariant  *target)
{
  const char *path;

  if (target == NULL)
    return NULL;

  path = g_variant_get_string (target, NULL);

  if (xd_remote_tree_is_remote_path (path))
    return self->remote != NULL ? xd_remote_tree_lookup (self->remote, path) : NULL;

  return xd_fs_tree_lookup (self->tree, path);
}

static void
create_folder (XdSidebar  *self,
               XdNode     *parent,
               const char *name)
{
  g_autoptr (GError) error = NULL;

  if (parent != NULL && is_remote_row (parent))
    {
      xd_remote_tree_create_folder (self->remote, parent, name);
      return;
    }

  if (xd_fs_tree_create_folder (self->tree, parent, name, &error) == NULL)
    show_error (self, "Could not create the folder", error);
}

static void
on_new_workspace (GtkWidget  *widget,
                  const char *action_name,
                  GVariant   *target)
{
  XdSidebar *self = XD_SIDEBAR (widget);

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
  XdSidebar *self = XD_SIDEBAR (widget);
  XdNode *parent = node_from_target (self, target);

  if (parent == NULL)
    return;

  prompt_for_name (self, "New Folder", NULL, "Create", NULL,
                   parent, create_folder);
}

static void
rename_folder (XdSidebar  *self,
               XdNode     *node,
               const char *name)
{
  g_autoptr (GError) error = NULL;

  if (is_remote_row (node))
    {
      xd_remote_tree_rename_folder (self->remote, node, name);
      return;
    }

  if (!xd_fs_tree_rename_folder (self->tree, node, name, &error))
    show_error (self, "Could not rename the folder", error);
}

static void
on_rename (GtkWidget  *widget,
           const char *action_name,
           GVariant   *target)
{
  XdSidebar *self = XD_SIDEBAR (widget);
  XdNode *node = node_from_target (self, target);

  if (node == NULL)
    return;

  prompt_for_name (self, "Rename Folder", NULL, "Rename",
                   xd_node_get_name (node), node, rename_folder);
}

/* --- chats ---------------------------------------------------------------- */

/* A chat id says nothing about where the chat is, so the local tree is asked
 * first and the remote answers for what it does not have. */
static XdNode *
chat_from_target (XdSidebar *self,
                  GVariant  *target)
{
  const char *chat_id;
  XdNode *chat;

  if (target == NULL)
    return NULL;

  chat_id = g_variant_get_string (target, NULL);

  chat = xd_fs_tree_lookup_chat (self->tree, chat_id);
  if (chat == NULL && self->remote != NULL)
    chat = xd_remote_tree_lookup_chat (self->remote, chat_id);

  return chat;
}

/*
 * New chats ask for a working directory, because a folder is an
 * organisational thing: "Lunar / Proxy" may want the proxy repo one day and a
 * scratch checkout the next, and neither of them lives inside the workspace
 * tree. Leaving it unset inherits the folder's own directory.
 */
typedef struct
{
  XdSidebar *self;
  XdNode *folder;           /* unowned; owned by the tree */
  GtkEditable *title_entry;
  AdwActionRow *dir_row;
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
update_dir_row (NewChatPrompt *prompt)
{
  if (prompt->workdir != NULL)
    adw_action_row_set_subtitle (prompt->dir_row, prompt->workdir);
  else
    adw_action_row_set_subtitle (prompt->dir_row, "Same as the folder");
}

static void
on_clear_directory (GtkButton *button,
                    gpointer   user_data)
{
  NewChatPrompt *prompt = user_data;

  g_clear_pointer (&prompt->workdir, g_free);
  update_dir_row (prompt);
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

  update_dir_row (prompt);
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
  g_autofree char *model = NULL;
  g_autofree char *effort = NULL;
  const char *response;
  const char *title;
  XdNode *chat;

  response = adw_alert_dialog_choose_finish (ADW_ALERT_DIALOG (source), result);

  if (g_strcmp0 (response, "confirm") != 0)
    {
      new_chat_prompt_free (prompt);
      return;
    }

  title = gtk_editable_get_text (prompt->title_entry);
  if (*title == '\0')
    title = "New Chat";

  /* On a remote the folder chain that decides all of this lives over there,
   * and so does the CLI that will answer: the daemon fills it in, and the new
   * chat comes back as a row to open. */
  if (is_remote_row (prompt->folder))
    {
      xd_remote_tree_create_chat (prompt->self->remote, prompt->folder, title);
      new_chat_prompt_free (prompt);
      return;
    }

  /* The backend is fixed when the chat is created, because the session id the
   * CLI hands back only means something to that CLI. */
  {
    g_autofree char *fallback =
      g_settings_get_string (prompt->self->settings, "default-backend");
    g_autoptr (XdEffectiveSettings) resolved =
      xd_settings_resolve (prompt->folder, fallback);
    const AiBackend *definition;

    backend = g_strdup (resolved->backend);

    /* Every chat names a model, so it can always say which one answered. The
     * folder chain gets to pick; failing that, the backend's newest. */
    definition = ai_backend_lookup (backend);
    if (resolved->model != NULL)
      model = g_strdup (resolved->model);
    else if (definition != NULL)
      model = g_strdup (definition->default_model);

    if (definition != NULL)
      effort = g_strdup (ai_effort_to_string (ai_backend_default_effort (definition)));
  }

  chat = xd_fs_tree_create_chat (prompt->self->tree, prompt->folder, title,
                                 backend, model, effort, prompt->workdir,
                                 &error);
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
  XdSidebar *self = XD_SIDEBAR (widget);
  XdNode *folder = node_from_target (self, target);
  NewChatPrompt *prompt;
  AdwAlertDialog *dialog;
  GtkWidget *group;
  GtkWidget *title_row;
  GtkWidget *choose;
  GtkWidget *clear;
  GtkWidget *buttons;

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

  /* Two rows in a boxed list: a name and where it runs. The directory is a
   * subtitle rather than a button the width of the dialog, because a path is
   * something to read, not something to press. */
  title_row = adw_entry_row_new ();
  adw_preferences_row_set_title (ADW_PREFERENCES_ROW (title_row), "Name");
  g_object_set (title_row, "activates-default", TRUE, NULL);
  prompt->title_entry = GTK_EDITABLE (title_row);

  group = adw_preferences_group_new ();
  adw_preferences_group_add (ADW_PREFERENCES_GROUP (group), title_row);

  /*
   * Only a name for a remote chat.
   *
   * The picker below chooses a directory on this machine, and the chat will
   * run on another one -- so there is nothing here worth offering. The daemon
   * takes the working directory from the folder, which is where a local chat
   * gets it too when nothing is picked.
   */
  if (!is_remote_row (folder))
    {
      prompt->dir_row = ADW_ACTION_ROW (adw_action_row_new ());
      adw_preferences_row_set_title (ADW_PREFERENCES_ROW (prompt->dir_row), "Runs in");
      adw_action_row_set_subtitle_lines (prompt->dir_row, 2);

      choose = gtk_button_new_from_icon_name ("folder-open-symbolic");
      gtk_widget_add_css_class (choose, "flat");
      gtk_widget_set_valign (choose, GTK_ALIGN_CENTER);
      gtk_widget_set_tooltip_text (choose, "Choose a directory…");
      g_signal_connect (choose, "clicked", G_CALLBACK (on_choose_directory), prompt);

      clear = gtk_button_new_from_icon_name ("edit-clear-symbolic");
      gtk_widget_add_css_class (clear, "flat");
      gtk_widget_set_valign (clear, GTK_ALIGN_CENTER);
      gtk_widget_set_tooltip_text (clear, "Use the folder's own directory");
      g_signal_connect (clear, "clicked", G_CALLBACK (on_clear_directory), prompt);

      buttons = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
      gtk_box_append (GTK_BOX (buttons), choose);
      gtk_box_append (GTK_BOX (buttons), clear);
      adw_action_row_add_suffix (prompt->dir_row, buttons);

      update_dir_row (prompt);

      adw_preferences_group_add (ADW_PREFERENCES_GROUP (group),
                                 GTK_WIDGET (prompt->dir_row));
    }

  adw_alert_dialog_set_extra_child (dialog, group);

  adw_alert_dialog_choose (dialog, GTK_WIDGET (self), NULL,
                           on_new_chat_response, prompt);
}

static void
rename_chat (XdSidebar  *self,
             XdNode     *chat,
             const char *title)
{
  g_autoptr (GError) error = NULL;

  if (is_remote_row (chat))
    {
      xd_remote_tree_rename_chat (self->remote, chat, title);
      return;
    }

  if (!xd_fs_tree_rename_chat (self->tree, chat, title, &error))
    show_error (self, "Could not rename the chat", error);
}

static void
on_rename_chat (GtkWidget  *widget,
                const char *action_name,
                GVariant   *target)
{
  XdSidebar *self = XD_SIDEBAR (widget);
  XdNode *chat = chat_from_target (self, target);

  if (chat == NULL)
    return;

  prompt_for_name (self, "Rename Chat", NULL, "Rename",
                   xd_node_get_name (chat), chat, rename_chat);
}

static void
on_delete_chat (GtkWidget  *widget,
                const char *action_name,
                GVariant   *target)
{
  XdSidebar *self = XD_SIDEBAR (widget);
  XdNode *chat = chat_from_target (self, target);
  g_autoptr (GError) error = NULL;

  if (chat == NULL)
    return;

  if (is_remote_row (chat))
    {
      xd_remote_tree_delete_chat (self->remote, chat);
      return;
    }

  if (!xd_fs_tree_delete_chat (self->tree, chat, &error))
    show_error (self, "Could not delete the chat", error);
}

/*
 * Reads the remote's tree again.
 *
 * It is read on its own whenever the connection comes up, and every change
 * made from here is followed by another read -- but a change made on the other
 * machine, or from a third one, arrives nowhere until the daemon can say so.
 * Until it can, this is how to ask.
 */
static void
on_refresh_remote (GtkWidget  *widget,
                   const char *action_name,
                   GVariant   *target)
{
  XdSidebar *self = XD_SIDEBAR (widget);

  if (self->remote != NULL)
    xd_remote_tree_refresh (self->remote);
}

static void
on_folder_settings (GtkWidget  *widget,
                    const char *action_name,
                    GVariant   *target)
{
  XdSidebar *self = XD_SIDEBAR (widget);
  XdNode *folder = node_from_target (self, target);

  if (folder == NULL)
    return;

  xd_folder_settings_dialog_present (GTK_WIDGET (self), folder, self->settings);
}

typedef struct
{
  XdSidebar *self;
  XdNode *node;
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

  if (g_strcmp0 (response, "trash") != 0)
    {
      g_object_unref (prompt->self);
      g_free (prompt);
      return;
    }

  if (is_remote_row (prompt->node))
    xd_remote_tree_trash_folder (prompt->self->remote, prompt->node);
  else if (!xd_fs_tree_trash_folder (prompt->self->tree, prompt->node, &error))
    show_error (prompt->self, "Could not move the folder to the trash", error);

  g_object_unref (prompt->self);
  g_free (prompt);
}

static void
on_trash (GtkWidget  *widget,
          const char *action_name,
          GVariant   *target)
{
  XdSidebar *self = XD_SIDEBAR (widget);
  XdNode *node = node_from_target (self, target);
  g_autofree char *body = NULL;
  AdwAlertDialog *dialog;
  TrashPrompt *prompt;

  if (node == NULL)
    return;

  body = g_strdup_printf ("“%s” and everything inside it will be moved to the "
                          "trash.", xd_node_get_name (node));

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
  XdNode *node = item;

  if (xd_node_get_kind (node) != XD_NODE_FOLDER)
    return NULL;

  return G_LIST_MODEL (g_object_ref (xd_node_get_children (node)));
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
  XdSidebar *self = user_data;
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
queue_save_expanded (XdSidebar *self)
{
  if (self->save_expanded_id == 0)
    self->save_expanded_id = g_idle_add (save_expanded, self);
}

static void
on_row_expanded (GtkTreeListRow *row,
                 GParamSpec     *pspec,
                 gpointer        user_data)
{
  XdSidebar *self = user_data;
  g_autoptr (XdNode) node = gtk_tree_list_row_get_item (row);
  const char *folder_id;

  if (node == NULL || xd_node_get_kind (node) != XD_NODE_FOLDER)
    return;

  folder_id = xd_node_get_folder_id (node);
  if (folder_id == NULL)
    return;

  if (gtk_tree_list_row_get_expanded (row))
    g_hash_table_add (self->expanded, g_strdup (folder_id));
  else
    g_hash_table_remove (self->expanded, folder_id);

  queue_save_expanded (self);
}

/* Right-clicking any row opens the same menu the folder button shows. */
static void
on_row_right_clicked (GtkGestureClick *gesture,
                      int              n_press,
                      double           x,
                      double           y,
                      gpointer         user_data)
{
  GtkPopover *popover = user_data;
  GdkRectangle at = { (int) x, (int) y, 1, 1 };

  if (gtk_popover_menu_get_menu_model (GTK_POPOVER_MENU (popover)) == NULL)
    return;

  gtk_popover_set_pointing_to (popover, &at);
  gtk_popover_popup (popover);
}

/*
 * Draws what a chat is doing.
 *
 * A spinner while it is working, since that is the one state that is going to
 * end on its own and a still picture cannot say "still going". A chat waiting
 * to be answered keeps its own icon but is marked as needing attention, which
 * the stylesheet pulses -- it is waiting for the user, so it should catch the
 * eye rather than sit still. Otherwise the assistant's icon, which is the
 * resting state and says who has been answering.
 */
static void
show_state (XdNode     *node,
            GParamSpec *pspec,
            gpointer    user_data)
{
  GtkWidget *box = user_data;
  GtkWidget *icon = g_object_get_data (G_OBJECT (box), "icon");
  GtkWidget *spinner = g_object_get_data (G_OBJECT (box), "spinner");
  XdNodeState state = xd_node_get_state (node);

  gtk_widget_set_visible (spinner, state == XD_NODE_WORKING);
  gtk_spinner_set_spinning (GTK_SPINNER (spinner), state == XD_NODE_WORKING);
  gtk_widget_set_visible (icon, state != XD_NODE_WORKING);

  if (state == XD_NODE_WAITING)
    gtk_widget_add_css_class (icon, "xd-waiting");
  else
    gtk_widget_remove_css_class (icon, "xd-waiting");
}

/* --- moving folders by dragging ------------------------------------------- */

/*
 * The node a row is showing, or NULL for the empty space below the tree.
 *
 * Read from the widget at drop time rather than captured when the row was
 * built: rows are recycled as the list scrolls, so a callback holding the
 * node it was bound with would be answering for a different one.
 */
static XdNode *
node_for_row (GtkWidget *widget)
{
  return g_object_get_data (G_OBJECT (widget), "node");
}

static GdkContentProvider *
on_drag_prepare (GtkDragSource *source,
                 double         x,
                 double         y,
                 gpointer       user_data)
{
  XdNode *node = node_for_row (user_data);

  /* Chats belong to whichever folder they were made in; only folders move. */
  if (node == NULL || xd_node_get_kind (node) != XD_NODE_FOLDER)
    return NULL;

  return gdk_content_provider_new_typed (XD_TYPE_NODE, node);
}

static void
on_drag_begin (GtkDragSource *source,
               GdkDrag       *drag,
               gpointer       user_data)
{
  GtkWidget *row = user_data;
  g_autoptr (GdkPaintable) paintable = gtk_widget_paintable_new (row);

  gtk_drag_source_set_icon (source, paintable, 0, 0);
}

static gboolean
on_drop (GtkDropTarget *target,
         const GValue  *value,
         double         x,
         double         y,
         gpointer       user_data)
{
  XdSidebar *self = g_object_get_data (G_OBJECT (target), "sidebar");
  XdNode *dropped = g_value_get_object (value);
  XdNode *onto = node_for_row (user_data);
  g_autoptr (GError) error = NULL;

  if (dropped == NULL)
    return FALSE;

  /* Dropped on a chat: it stands for the folder holding it, which is what
   * the row looks like it is part of. */
  if (onto != NULL && xd_node_get_kind (onto) != XD_NODE_FOLDER)
    onto = xd_node_get_parent (onto);

  /*
   * Each side stays on its own machine.
   *
   * Dragging a row moves a directory, and a directory cannot be moved to
   * another computer by moving it: that would be a copy, over the wire, of
   * something that may be a repository. Refused rather than half-done.
   *
   * The empty space below the tree is the local top level, so a remote folder
   * dropped there is a folder dragged out of the remote entirely.
   */
  if (is_remote_row (dropped) != (onto != NULL && is_remote_row (onto)))
    return FALSE;

  if (is_remote_row (dropped))
    {
      xd_remote_tree_move_folder (self->remote, dropped, onto);
      return TRUE;
    }

  if (!xd_fs_tree_move_folder (self->tree, dropped, onto, &error))
    {
      /* Refusing to move is normal here -- into itself, onto a name already
       * taken -- so it is worth saying why rather than doing nothing. */
      show_error (self, "Cannot Move the Folder", error);
      return FALSE;
    }

  return TRUE;
}

/*
 * Answers for a row, or for the empty space below the tree.
 *
 * The empty space is how a folder gets back out to the top level: there is
 * no row standing for "not in any folder" to drop it on.
 */
static void
add_drop_target (XdSidebar *self,
                 GtkWidget *widget)
{
  GtkDropTarget *target = gtk_drop_target_new (XD_TYPE_NODE, GDK_ACTION_MOVE);

  g_object_set_data (G_OBJECT (target), "sidebar", self);
  g_signal_connect (target, "drop", G_CALLBACK (on_drop), widget);
  gtk_widget_add_controller (widget, GTK_EVENT_CONTROLLER (target));
}

static void
on_item_setup (GtkSignalListItemFactory *factory,
               GtkListItem              *item,
               gpointer                  user_data)
{
  GtkWidget *expander = gtk_tree_expander_new ();
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *icon = gtk_image_new ();
  GtkWidget *spinner = gtk_spinner_new ();
  GtkWidget *label = gtk_label_new (NULL);
  GtkWidget *popover = gtk_popover_menu_new_from_model (NULL);

  gtk_widget_add_css_class (popover, "xd-menu-popover");
  GtkGesture *gesture;

  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_label_set_ellipsize (GTK_LABEL (label), PANGO_ELLIPSIZE_END);
  gtk_widget_set_hexpand (label, TRUE);

  gtk_box_append (GTK_BOX (box), icon);
  gtk_box_append (GTK_BOX (box), spinner);
  gtk_box_append (GTK_BOX (box), label);

  gtk_widget_set_parent (popover, box);
  gtk_popover_set_has_arrow (GTK_POPOVER (popover), FALSE);
  gtk_widget_set_halign (popover, GTK_ALIGN_START);
  g_object_set_data (G_OBJECT (item), "context-menu", popover);

  gesture = gtk_gesture_click_new ();
  gtk_gesture_single_set_button (GTK_GESTURE_SINGLE (gesture), GDK_BUTTON_SECONDARY);
  g_signal_connect (gesture, "pressed", G_CALLBACK (on_row_right_clicked), popover);
  gtk_widget_add_controller (box, GTK_EVENT_CONTROLLER (gesture));

  {
    GtkDragSource *source = gtk_drag_source_new ();

    gtk_drag_source_set_actions (source, GDK_ACTION_MOVE);
    g_signal_connect (source, "prepare", G_CALLBACK (on_drag_prepare), box);
    g_signal_connect (source, "drag-begin", G_CALLBACK (on_drag_begin), box);
    gtk_widget_add_controller (box, GTK_EVENT_CONTROLLER (source));

    add_drop_target (user_data, box);
  }

  gtk_tree_expander_set_child (GTK_TREE_EXPANDER (expander), box);
  gtk_list_item_set_child (item, expander);
}

/* A popover added with set_parent() has to be taken back off by hand. */
static void
on_item_teardown (GtkSignalListItemFactory *factory,
                  GtkListItem              *item,
                  gpointer                  user_data)
{
  GtkWidget *popover = g_object_get_data (G_OBJECT (item), "context-menu");

  if (popover != NULL)
    {
      gtk_widget_unparent (popover);
      g_object_set_data (G_OBJECT (item), "context-menu", NULL);
    }
}

static GMenuModel *
build_row_menu (XdNode *node)
{
  g_autoptr (GVariant) target =
    g_variant_ref_sink (g_variant_new_string (xd_node_get_path (node)));
  gboolean remote = is_remote_row (node);
  GMenu *menu = g_menu_new ();
  GMenu *section = g_menu_new ();
  g_autoptr (GMenuItem) new_chat = NULL;
  g_autoptr (GMenuItem) new_folder = NULL;
  g_autoptr (GMenuItem) rename = NULL;
  g_autoptr (GMenuItem) settings = NULL;
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

  /* The settings dialog edits the dotfile inside the folder, which for a
   * remote is a file on the other machine that nothing here can write. */
  if (!remote)
    {
      settings = g_menu_item_new ("Folder Settings…", NULL);
      g_menu_item_set_action_and_target_value (settings, "sidebar.settings", target);
      g_menu_append_item (menu, settings);
    }

  trash = g_menu_item_new ("Move to Trash", NULL);
  g_menu_item_set_action_and_target_value (trash, "sidebar.trash", target);
  g_menu_append_item (section, trash);
  g_menu_append_section (menu, NULL, G_MENU_MODEL (section));
  g_object_unref (section);

  return G_MENU_MODEL (menu);
}

/*
 * The remote's own row, which is the machine rather than a folder on it.
 *
 * It can hold new workspaces and it can be refreshed; it cannot be renamed or
 * thrown away, because neither means anything to do to a computer.
 */
static GMenuModel *
build_remote_menu (XdNode *node)
{
  g_autoptr (GVariant) target =
    g_variant_ref_sink (g_variant_new_string (xd_node_get_path (node)));
  GMenu *menu = g_menu_new ();
  g_autoptr (GMenuItem) new_folder = NULL;
  g_autoptr (GMenuItem) refresh = NULL;

  new_folder = g_menu_item_new ("New Workspace", NULL);
  g_menu_item_set_action_and_target_value (new_folder, "sidebar.new-folder", target);
  g_menu_append_item (menu, new_folder);

  refresh = g_menu_item_new ("Refresh", NULL);
  g_menu_item_set_action_and_target_value (refresh, "sidebar.refresh-remote", target);
  g_menu_append_item (menu, refresh);

  return G_MENU_MODEL (menu);
}

static GMenuModel *
build_chat_menu (XdNode *node)
{
  g_autoptr (GVariant) target =
    g_variant_ref_sink (g_variant_new_string (xd_node_get_chat_id (node)));
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
  XdSidebar *self = user_data;
  GtkTreeListRow *row = gtk_list_item_get_item (item);
  GtkWidget *expander = gtk_list_item_get_child (item);
  GtkWidget *box = gtk_tree_expander_get_child (GTK_TREE_EXPANDER (expander));
  GtkWidget *icon = gtk_widget_get_first_child (box);
  GtkWidget *spinner = gtk_widget_get_next_sibling (icon);
  GtkWidget *label = gtk_widget_get_next_sibling (spinner);
  g_autoptr (XdNode) node = gtk_tree_list_row_get_item (row);

  gtk_tree_expander_set_list_row (GTK_TREE_EXPANDER (expander), row);
  gtk_image_set_from_icon_name (GTK_IMAGE (icon), xd_node_get_icon_name (node));

  g_object_set_data (G_OBJECT (item), "icon-binding",
                     g_object_bind_property (node, "icon-name", icon, "icon-name",
                                             G_BINDING_SYNC_CREATE));

  g_object_set_data (G_OBJECT (box), "icon", icon);
  g_object_set_data (G_OBJECT (box), "spinner", spinner);
  show_state (node, NULL, box);
  g_signal_connect (node, "notify::state", G_CALLBACK (show_state), box);
  g_object_set_data_full (G_OBJECT (item), "state-watch", g_object_ref (node),
                          g_object_unref);

  /* Read back by the drag handlers, which run long after this returns, and
   * survives the row being recycled onto a different node. */
  g_object_set_data_full (G_OBJECT (box), "node", g_object_ref (node),
                          g_object_unref);

  g_object_set_data (G_OBJECT (item), "name-binding",
                     g_object_bind_property (node, "name", label, "label",
                                             G_BINDING_SYNC_CREATE));

  {
    gboolean is_folder = xd_node_get_kind (node) == XD_NODE_FOLDER;
    gboolean is_remote_root = self->remote != NULL &&
      node == xd_remote_tree_get_root (self->remote);
    GtkWidget *popover = g_object_get_data (G_OBJECT (item), "context-menu");
    g_autoptr (GMenuModel) menu = NULL;

    /* The same menu wherever the row lives: what each item does is settled
     * when it is chosen, by which tree the row came from. */
    if (is_remote_root)
      menu = build_remote_menu (node);
    else
      menu = is_folder ? build_row_menu (node) : build_chat_menu (node);

    /* Right-click is the only way in, for folders as for chats: a button on
     * every folder row was a column of dots down the tree. */
    gtk_popover_menu_set_menu_model (GTK_POPOVER_MENU (popover), menu);
  }

  if (xd_node_get_kind (node) == XD_NODE_FOLDER)
    {
      const char *folder_id = xd_node_get_folder_id (node);
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
  GBinding *icon_binding = g_object_get_data (G_OBJECT (item), "icon-binding");
  XdNode *watched = g_object_get_data (G_OBJECT (item), "state-watch");
  gpointer handler = g_object_get_data (G_OBJECT (item), "expanded-handler");
  GtkTreeListRow *row = gtk_list_item_get_item (item);

  if (icon_binding != NULL)
    {
      g_binding_unbind (icon_binding);
      g_object_set_data (G_OBJECT (item), "icon-binding", NULL);
    }

  if (watched != NULL)
    {
      GtkWidget *expander = gtk_list_item_get_child (item);
      GtkWidget *box = gtk_tree_expander_get_child (GTK_TREE_EXPANDER (expander));

      g_signal_handlers_disconnect_by_func (watched, G_CALLBACK (show_state), box);
      g_object_set_data (G_OBJECT (item), "state-watch", NULL);
    }

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
  XdSidebar *self = user_data;
  GtkTreeListRow *row = gtk_single_selection_get_selected_item (selection);
  g_autoptr (XdNode) node = NULL;

  if (row != NULL)
    node = gtk_tree_list_row_get_item (row);

  g_signal_emit (self, signals[SIGNAL_NODE_SELECTED], 0, node);
}

static void
on_row_activated (GtkListView *list_view,
                  guint        position,
                  gpointer     user_data)
{
  XdSidebar *self = user_data;
  g_autoptr (GtkTreeListRow) row = NULL;
  g_autoptr (XdNode) node = NULL;

  row = g_list_model_get_item (G_LIST_MODEL (self->selection), position);
  if (row == NULL)
    return;

  node = gtk_tree_list_row_get_item (row);

  /* Double-clicking a folder is the natural "open/close" gesture. */
  if (xd_node_get_kind (node) == XD_NODE_FOLDER)
    gtk_tree_list_row_set_expanded (row, !gtk_tree_list_row_get_expanded (row));
  else
    g_signal_emit (self, signals[SIGNAL_NODE_ACTIVATED], 0, node);
}

/* --- construction --------------------------------------------------------- */

XdSidebar *
xd_sidebar_new (XdFsTree *tree)
{
  GtkFlattenListModel *top_level;
  XdSidebar *self;

  g_return_val_if_fail (XD_IS_FS_TREE (tree), NULL);

  self = g_object_new (XD_TYPE_SIDEBAR, NULL);
  self->tree = g_object_ref (tree);

  /*
   * The top level is a list of lists: the local workspaces, and a remote's own
   * root after them. Flattening rather than copying keeps each tree the owner
   * of its rows, so a remote appearing or going away is one model coming and
   * going rather than the tree being rebuilt around it.
   */
  self->roots = g_list_store_new (G_TYPE_LIST_MODEL);
  g_list_store_append (self->roots, xd_fs_tree_get_model (tree));

  top_level = gtk_flatten_list_model_new (g_object_ref (G_LIST_MODEL (self->roots)));

  self->tree_model = gtk_tree_list_model_new (G_LIST_MODEL (top_level),
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

/* The daemon would not do what was asked. Nothing changed on either side, so
 * saying so is the whole of it. */
static void
on_remote_failed (XdRemoteTree *remote,
                  const char   *heading,
                  const char   *message,
                  gpointer      user_data)
{
  show_error_message (user_data, heading, message);
}

/* A chat made on the daemon has arrived in the tree; opening it is the rest of
 * what "New Chat" meant. */
static void
on_remote_chat_created (XdRemoteTree *remote,
                        XdNode       *chat,
                        gpointer      user_data)
{
  XdSidebar *self = user_data;

  g_signal_emit (self, signals[SIGNAL_NODE_ACTIVATED], 0, chat);
}

void
xd_sidebar_set_remote (XdSidebar    *self,
                       XdRemoteTree *remote)
{
  guint position;

  g_return_if_fail (XD_IS_SIDEBAR (self));

  if (self->remote == remote)
    return;

  if (self->remote != NULL)
    {
      g_signal_handlers_disconnect_by_data (self->remote, self);

      if (g_list_store_find (self->roots,
                             xd_remote_tree_get_model (self->remote), &position))
        g_list_store_remove (self->roots, position);
    }

  g_set_object (&self->remote, remote);

  if (remote == NULL)
    return;

  g_signal_connect (remote, "failed", G_CALLBACK (on_remote_failed), self);
  g_signal_connect (remote, "chat-created",
                    G_CALLBACK (on_remote_chat_created), self);

  /* After the local workspaces, so the tree reads as "what is here, then what
   * is over there". */
  g_list_store_append (self->roots, xd_remote_tree_get_model (remote));
}

static void
xd_sidebar_dispose (GObject *object)
{
  XdSidebar *self = XD_SIDEBAR (object);

  if (self->save_expanded_id != 0)
    {
      g_clear_handle_id (&self->save_expanded_id, g_source_remove);
      save_expanded (self);
    }

  if (self->remote != NULL)
    g_signal_handlers_disconnect_by_data (self->remote, self);

  g_clear_pointer (&self->expanded, g_hash_table_unref);
  g_clear_object (&self->selection);
  g_clear_object (&self->tree_model);
  g_clear_object (&self->roots);
  g_clear_object (&self->settings);
  g_clear_object (&self->remote);
  g_clear_object (&self->tree);

  G_OBJECT_CLASS (xd_sidebar_parent_class)->dispose (object);
}

static void
xd_sidebar_class_init (XdSidebarClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);
  GtkWidgetClass *widget_class = GTK_WIDGET_CLASS (klass);

  object_class->dispose = xd_sidebar_dispose;

  signals[SIGNAL_NODE_SELECTED] =
    g_signal_new ("node-selected", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, XD_TYPE_NODE);

  signals[SIGNAL_NODE_ACTIVATED] =
    g_signal_new ("node-activated", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, XD_TYPE_NODE);

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
  gtk_widget_class_install_action (widget_class, "sidebar.settings", "s",
                                   on_folder_settings);
  gtk_widget_class_install_action (widget_class, "sidebar.refresh-remote", "s",
                                   on_refresh_remote);
}

static void
xd_sidebar_init (XdSidebar *self)
{
  GtkWidget *toolbar = adw_toolbar_view_new ();
  GtkWidget *header = adw_header_bar_new ();
  GtkWidget *new_button = gtk_button_new_from_icon_name ("list-add-symbolic");
  GtkWidget *pair_button = gtk_button_new_from_icon_name ("network-server-symbolic");
  GtkWidget *scrolled = gtk_scrolled_window_new ();
  GtkListItemFactory *factory = gtk_signal_list_item_factory_new ();

  g_auto (GStrv) expanded = NULL;

  self->settings = g_settings_new (XD_APP_ID);
  self->expanded = g_hash_table_new_full (g_str_hash, g_str_equal, g_free, NULL);

  expanded = g_settings_get_strv (self->settings, "expanded-folders");
  for (gsize i = 0; expanded[i] != NULL; i++)
    g_hash_table_add (self->expanded, g_strdup (expanded[i]));

  gtk_widget_set_tooltip_text (new_button, "New Workspace");
  gtk_actionable_set_action_name (GTK_ACTIONABLE (new_button), "sidebar.new-workspace");
  adw_header_bar_pack_start (ADW_HEADER_BAR (header), new_button);

  /* The window's action, not the sidebar's: pairing sets up a connection the
   * whole window works through, and it is the window that owns it. It sits
   * here because this is where the remote turns up. */
  gtk_widget_set_tooltip_text (pair_button, "Connect to a Remote…");
  gtk_actionable_set_action_name (GTK_ACTIONABLE (pair_button), "win.pair-remote");
  adw_header_bar_pack_end (ADW_HEADER_BAR (header), pair_button);

  /*
   * The window's own title and buttons belong to the chat side.
   *
   * A header bar shows the window controls unless told otherwise, and the
   * sidebar sits beside another header bar rather than under it -- so without
   * this the window gets a second set of close buttons, over the tree.
   */
  adw_header_bar_set_title_widget (ADW_HEADER_BAR (header),
                                   adw_window_title_new ("Workspaces", NULL));
  adw_header_bar_set_show_end_title_buttons (ADW_HEADER_BAR (header), FALSE);
  adw_toolbar_view_add_top_bar (ADW_TOOLBAR_VIEW (toolbar), header);

  g_signal_connect (factory, "setup", G_CALLBACK (on_item_setup), self);
  g_signal_connect (factory, "bind", G_CALLBACK (on_item_bind), self);
  g_signal_connect (factory, "unbind", G_CALLBACK (on_item_unbind), self);
  g_signal_connect (factory, "teardown", G_CALLBACK (on_item_teardown), self);

  self->list_view = GTK_LIST_VIEW (gtk_list_view_new (NULL, factory));
  gtk_list_view_set_single_click_activate (self->list_view, FALSE);
  gtk_widget_add_css_class (GTK_WIDGET (self->list_view), "navigation-sidebar");

  /* Scrolls, but shows nothing for it: the tree is short and a bar down its
   * side is a line the eye keeps returning to. */
  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scrolled),
                                  GTK_POLICY_NEVER, GTK_POLICY_EXTERNAL);
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (scrolled),
                                 GTK_WIDGET (self->list_view));

  /* Nothing under the last row stands for the top level, so the empty space
   * itself is the way back out of a folder. */
  add_drop_target (self, GTK_WIDGET (self->list_view));
  gtk_widget_set_vexpand (scrolled, TRUE);

  adw_toolbar_view_set_content (ADW_TOOLBAR_VIEW (toolbar), scrolled);
  gtk_widget_add_css_class (toolbar, "xd-sidebar");
  gtk_widget_add_css_class (scrolled, "xd-sidebar");
  gtk_widget_add_css_class (GTK_WIDGET (self->list_view), "xd-sidebar");

  adw_bin_set_child (ADW_BIN (self), toolbar);
}
