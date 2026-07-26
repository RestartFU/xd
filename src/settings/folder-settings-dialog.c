#include "folder-settings-dialog.h"

#include "backend/backend.h"
#include "folder-settings.h"
#include "settings-resolver.h"

typedef struct
{
  XdNode *folder;                 /* unowned; owned by the tree */
  GSettings *app_settings;
  XdFolderSettings *settings;     /* the folder's own, being edited */
  XdEffectiveSettings *inherited; /* the parent chain's, for the subtitles */

  AdwComboRow *backend_row;
  AdwEntryRow *model_row;
  AdwActionRow *workdir_row;
  AdwActionRow *repo_row;
  GtkTextView *instructions;

  char *workdir;                  /* NULL: inherit */
  char *repo;
} Editor;

static void
editor_free (gpointer data)
{
  Editor *editor = data;

  g_clear_object (&editor->app_settings);
  g_clear_pointer (&editor->settings, xd_folder_settings_free);
  g_clear_pointer (&editor->inherited, xd_effective_settings_free);
  g_free (editor->workdir);
  g_free (editor->repo);
  g_free (editor);
}

/* --- directory rows ------------------------------------------------------- */

static void
update_path_row (AdwActionRow *row,
                 const char   *value,
                 const char   *inherited,
                 const char   *inherited_from)
{
  if (value != NULL && *value != '\0')
    {
      adw_action_row_set_subtitle (row, value);
      return;
    }

  if (inherited != NULL && inherited_from != NULL)
    {
      g_autofree char *text = g_strdup_printf ("%s — inherited from %s",
                                               inherited, inherited_from);

      adw_action_row_set_subtitle (row, text);
      return;
    }

  adw_action_row_set_subtitle (row, inherited != NULL ? inherited : "Not set");
}

static void
refresh_paths (Editor *editor)
{
  update_path_row (editor->workdir_row, editor->workdir,
                   editor->inherited->workdir, editor->inherited->workdir_from);
  update_path_row (editor->repo_row, editor->repo,
                   editor->inherited->repo, editor->inherited->repo_from);
}

typedef struct
{
  Editor *editor;
  gboolean is_repo;
} PathChoice;

static void
on_path_chosen (GObject      *source,
                GAsyncResult *result,
                gpointer      user_data)
{
  PathChoice *choice = user_data;
  g_autoptr (GFile) folder = NULL;

  folder = gtk_file_dialog_select_folder_finish (GTK_FILE_DIALOG (source),
                                                 result, NULL);
  if (folder != NULL)
    {
      char **slot = choice->is_repo ? &choice->editor->repo
                                    : &choice->editor->workdir;

      g_free (*slot);
      *slot = g_file_get_path (folder);

      refresh_paths (choice->editor);
    }

  g_free (choice);
}

static void
choose_path (Editor   *editor,
             GtkWidget *parent,
             gboolean  is_repo)
{
  g_autoptr (GtkFileDialog) dialog = gtk_file_dialog_new ();
  g_autofree char *projects_root = NULL;
  PathChoice *choice;

  gtk_file_dialog_set_title (dialog, is_repo ? "Repository" : "Working Directory");

  /* Repositories live outside the workspace tree, so start where they are. */
  projects_root = g_settings_get_string (editor->app_settings, "projects-root");
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

  choice = g_new0 (PathChoice, 1);
  choice->editor = editor;
  choice->is_repo = is_repo;

  gtk_file_dialog_select_folder (dialog, GTK_WINDOW (gtk_widget_get_root (parent)),
                                 NULL, on_path_chosen, choice);
}

static void
on_choose_workdir (GtkButton *button,
                   gpointer   user_data)
{
  choose_path (user_data, GTK_WIDGET (button), FALSE);
}

static void
on_choose_repo (GtkButton *button,
                gpointer   user_data)
{
  choose_path (user_data, GTK_WIDGET (button), TRUE);
}

static void
on_clear_workdir (GtkButton *button,
                  gpointer   user_data)
{
  Editor *editor = user_data;

  g_clear_pointer (&editor->workdir, g_free);
  refresh_paths (editor);
}

static void
on_clear_repo (GtkButton *button,
               gpointer   user_data)
{
  Editor *editor = user_data;

  g_clear_pointer (&editor->repo, g_free);
  refresh_paths (editor);
}

static AdwActionRow *
build_path_row (Editor      *editor,
                const char  *title,
                GCallback    choose,
                GCallback    clear)
{
  AdwActionRow *row = ADW_ACTION_ROW (adw_action_row_new ());
  GtkWidget *buttons = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
  GtkWidget *choose_button = gtk_button_new_from_icon_name ("folder-open-symbolic");
  GtkWidget *clear_button = gtk_button_new_from_icon_name ("edit-clear-symbolic");

  adw_preferences_row_set_title (ADW_PREFERENCES_ROW (row), title);
  adw_action_row_set_subtitle_lines (row, 2);

  gtk_widget_add_css_class (choose_button, "flat");
  gtk_widget_add_css_class (clear_button, "flat");
  gtk_widget_set_valign (choose_button, GTK_ALIGN_CENTER);
  gtk_widget_set_valign (clear_button, GTK_ALIGN_CENTER);
  gtk_widget_set_tooltip_text (choose_button, "Choose…");
  gtk_widget_set_tooltip_text (clear_button, "Inherit from the parent folder");

  g_signal_connect (choose_button, "clicked", choose, editor);
  g_signal_connect (clear_button, "clicked", clear, editor);

  gtk_box_append (GTK_BOX (buttons), choose_button);
  gtk_box_append (GTK_BOX (buttons), clear_button);
  adw_action_row_add_suffix (row, buttons);

  return row;
}

/* --- saving --------------------------------------------------------------- */

static char *
text_view_take_text (GtkTextView *view)
{
  GtkTextBuffer *buffer = gtk_text_view_get_buffer (view);
  GtkTextIter start, end;
  g_autofree char *text = NULL;

  gtk_text_buffer_get_bounds (buffer, &start, &end);
  text = gtk_text_buffer_get_text (buffer, &start, &end, FALSE);
  g_strstrip (text);

  return *text != '\0' ? g_steal_pointer (&text) : NULL;
}

static void
on_dialog_closed (AdwDialog *dialog,
                  gpointer   user_data)
{
  Editor *editor = user_data;
  g_autoptr (GError) error = NULL;
  const char *model;
  guint selected;

  /* Index 0 is "Inherit"; the rest follow the backend registry. */
  selected = adw_combo_row_get_selected (editor->backend_row);
  g_clear_pointer (&editor->settings->backend, g_free);
  if (selected > 0)
    {
      guint n_backends;
      const AiBackend *const *backends = ai_backend_all (&n_backends);

      if (selected - 1 < n_backends)
        editor->settings->backend = g_strdup (backends[selected - 1]->id);
    }

  model = gtk_editable_get_text (GTK_EDITABLE (editor->model_row));
  g_clear_pointer (&editor->settings->model, g_free);
  if (model != NULL && *model != '\0')
    editor->settings->model = g_strdup (model);

  g_clear_pointer (&editor->settings->workdir, g_free);
  editor->settings->workdir = g_strdup (editor->workdir);

  g_clear_pointer (&editor->settings->repo, g_free);
  editor->settings->repo = g_strdup (editor->repo);

  g_clear_pointer (&editor->settings->instructions, g_free);
  editor->settings->instructions = text_view_take_text (editor->instructions);

  if (!xd_folder_settings_save (editor->settings, xd_node_get_path (editor->folder),
                                &error))
    g_warning ("cannot save folder settings: %s", error->message);
}

/* --- construction --------------------------------------------------------- */

void
xd_folder_settings_dialog_present (GtkWidget *parent,
                                   XdNode    *folder,
                                   GSettings *app_settings)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GtkStringList) backend_names = NULL;
  g_autofree char *default_backend = NULL;
  AdwPreferencesDialog *dialog;
  AdwPreferencesPage *page;
  AdwPreferencesGroup *backend_group;
  AdwPreferencesGroup *project_group;
  AdwPreferencesGroup *instructions_group;
  GtkWidget *frame;
  GtkWidget *scroller;
  const AiBackend *const *backends;
  guint n_backends;
  Editor *editor;

  g_return_if_fail (XD_IS_NODE (folder));
  g_return_if_fail (xd_node_get_kind (folder) == XD_NODE_FOLDER);

  editor = g_new0 (Editor, 1);
  editor->folder = folder;
  editor->app_settings = g_object_ref (app_settings);

  editor->settings = xd_folder_settings_ensure (xd_node_get_path (folder), &error);
  if (editor->settings == NULL)
    {
      g_warning ("cannot read folder settings: %s", error->message);
      editor_free (editor);
      return;
    }

  /* Resolved from the parent up, so the subtitles describe what this folder
   * would get if it set nothing itself. */
  default_backend = g_settings_get_string (app_settings, "default-backend");
  editor->inherited = xd_settings_resolve (xd_node_get_parent (folder),
                                           default_backend);

  editor->workdir = g_strdup (editor->settings->workdir);
  editor->repo = g_strdup (editor->settings->repo);

  dialog = ADW_PREFERENCES_DIALOG (adw_preferences_dialog_new ());
  adw_dialog_set_title (ADW_DIALOG (dialog), xd_node_get_name (folder));

  page = ADW_PREFERENCES_PAGE (adw_preferences_page_new ());

  /* --- backend --- */
  backend_group = ADW_PREFERENCES_GROUP (adw_preferences_group_new ());
  adw_preferences_group_set_title (backend_group, "Assistant");
  adw_preferences_group_set_description (backend_group,
                                         "Chats started in this folder use "
                                         "these unless they say otherwise.");

  backends = ai_backend_all (&n_backends);
  backend_names = gtk_string_list_new (NULL);
  {
    g_autofree char *inherit_label =
      g_strdup_printf ("Inherit (%s)", editor->inherited->backend);

    gtk_string_list_append (backend_names, inherit_label);
  }
  for (guint i = 0; i < n_backends; i++)
    gtk_string_list_append (backend_names, backends[i]->display_name);

  editor->backend_row = ADW_COMBO_ROW (adw_combo_row_new ());
  adw_preferences_row_set_title (ADW_PREFERENCES_ROW (editor->backend_row), "Backend");
  adw_combo_row_set_model (editor->backend_row,
                           G_LIST_MODEL (g_object_ref (backend_names)));

  adw_combo_row_set_selected (editor->backend_row, 0);
  for (guint i = 0; i < n_backends; i++)
    {
      if (g_strcmp0 (editor->settings->backend, backends[i]->id) == 0)
        adw_combo_row_set_selected (editor->backend_row, i + 1);
    }

  editor->model_row = ADW_ENTRY_ROW (adw_entry_row_new ());
  adw_preferences_row_set_title (ADW_PREFERENCES_ROW (editor->model_row),
                                 editor->inherited->model != NULL
                                   ? "Model (blank inherits)" : "Model (blank: CLI default)");
  if (editor->settings->model != NULL)
    gtk_editable_set_text (GTK_EDITABLE (editor->model_row), editor->settings->model);

  adw_preferences_group_add (backend_group, GTK_WIDGET (editor->backend_row));
  adw_preferences_group_add (backend_group, GTK_WIDGET (editor->model_row));
  adw_preferences_page_add (page, backend_group);

  /* --- project --- */
  project_group = ADW_PREFERENCES_GROUP (adw_preferences_group_new ());
  adw_preferences_group_set_title (project_group, "Project");
  adw_preferences_group_set_description (project_group,
                                         "Where the assistant runs. The code "
                                         "does not have to live inside the "
                                         "workspace tree.");

  editor->workdir_row = build_path_row (editor, "Working Directory",
                                        G_CALLBACK (on_choose_workdir),
                                        G_CALLBACK (on_clear_workdir));
  editor->repo_row = build_path_row (editor, "Repository",
                                     G_CALLBACK (on_choose_repo),
                                     G_CALLBACK (on_clear_repo));

  adw_preferences_group_add (project_group, GTK_WIDGET (editor->workdir_row));
  adw_preferences_group_add (project_group, GTK_WIDGET (editor->repo_row));
  adw_preferences_page_add (page, project_group);

  refresh_paths (editor);

  /* --- instructions --- */
  instructions_group = ADW_PREFERENCES_GROUP (adw_preferences_group_new ());
  adw_preferences_group_set_title (instructions_group, "Instructions");
  adw_preferences_group_set_description (instructions_group,
                                         "Added to every chat in this folder, "
                                         "after any the parent folders set.");

  editor->instructions = GTK_TEXT_VIEW (gtk_text_view_new ());
  gtk_text_view_set_wrap_mode (editor->instructions, GTK_WRAP_WORD_CHAR);
  gtk_text_view_set_top_margin (editor->instructions, 8);
  gtk_text_view_set_bottom_margin (editor->instructions, 8);
  gtk_text_view_set_left_margin (editor->instructions, 8);
  gtk_text_view_set_right_margin (editor->instructions, 8);

  if (editor->settings->instructions != NULL)
    gtk_text_buffer_set_text (gtk_text_view_get_buffer (editor->instructions),
                              editor->settings->instructions, -1);

  scroller = gtk_scrolled_window_new ();
  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scroller),
                                  GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
  gtk_scrolled_window_set_min_content_height (GTK_SCROLLED_WINDOW (scroller), 140);
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (scroller),
                                 GTK_WIDGET (editor->instructions));

  frame = gtk_frame_new (NULL);
  gtk_frame_set_child (GTK_FRAME (frame), scroller);
  adw_preferences_group_add (instructions_group, frame);
  adw_preferences_page_add (page, instructions_group);

  adw_preferences_dialog_add (dialog, page);

  g_signal_connect (dialog, "closed", G_CALLBACK (on_dialog_closed), editor);
  g_object_set_data_full (G_OBJECT (dialog), "editor", editor, editor_free);

  adw_dialog_present (ADW_DIALOG (dialog), parent);
}
