#include "file-pane.h"

#include <string.h>

#define FILE_PREVIEW_LIMIT (1024 * 1024)

typedef struct
{
  char *name;
  gboolean directory;
} FileEntry;

typedef struct
{
  XdFilePane *pane;
  char *path;
  GFileInputStream *stream;
} FileRequest;

struct _XdFilePane
{
  AdwBin parent_instance;

  char *workdir;
  char *path;                 /* relative to the chat's working directory */
  XdRemoteClient *remote;
  char *chat_id;
  GCancellable *cancellable;

  GtkButton *back;
  GtkLabel *path_label;
  GtkListBox *entries;
  GtkStack *stack;
  AdwStatusPage *status;
  GtkTextBuffer *preview;
  gboolean showing_preview;
};

G_DEFINE_FINAL_TYPE (XdFilePane, xd_file_pane, ADW_TYPE_BIN)

static void show_directory (XdFilePane *self, const char *path);
static void show_file (XdFilePane *self, const char *path);

static void
file_entry_free (FileEntry *entry)
{
  g_free (entry->name);
  g_free (entry);
}

static gint
compare_entries (gconstpointer a,
                 gconstpointer b)
{
  const FileEntry *left = a;
  const FileEntry *right = b;

  if (left->directory != right->directory)
    return left->directory ? -1 : 1;

  return g_utf8_collate (left->name, right->name);
}

static FileRequest *
file_request_new (XdFilePane *self,
                  const char *path)
{
  FileRequest *request = g_new0 (FileRequest, 1);

  request->pane = g_object_ref (self);
  request->path = g_strdup (path != NULL ? path : "");
  return request;
}

static void
file_request_free (FileRequest *request)
{
  g_clear_object (&request->stream);
  g_clear_object (&request->pane);
  g_free (request->path);
  g_free (request);
}

static void
reset_request (XdFilePane *self)
{
  g_cancellable_cancel (self->cancellable);
  g_clear_object (&self->cancellable);
  self->cancellable = g_cancellable_new ();
}

static char *
child_path (const char *parent,
            const char *name)
{
  return parent == NULL || *parent == '\0'
    ? g_strdup (name)
    : g_strdup_printf ("%s/%s", parent, name);
}

static char *
parent_path (const char *path)
{
  g_autofree char *copy = g_strdup (path != NULL ? path : "");
  char *slash;

  slash = strrchr (copy, '/');
  if (slash == NULL)
    return g_strdup ("");

  *slash = '\0';
  return g_steal_pointer (&copy);
}

static char *
local_path (XdFilePane *self,
            const char *relative)
{
  if (relative == NULL || *relative == '\0')
    return g_strdup (self->workdir);

  return g_build_filename (self->workdir, relative, NULL);
}

static void
set_header_path (XdFilePane *self,
                 const char *path)
{
  g_autofree char *label = NULL;

  if (path == NULL || *path == '\0')
    label = self->workdir != NULL
      ? g_path_get_basename (self->workdir) : g_strdup ("Files");
  else
    label = g_strdup (path);

  gtk_label_set_label (self->path_label, label);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->path_label), label);
  gtk_widget_set_sensitive (GTK_WIDGET (self->back),
                            self->showing_preview ||
                            (self->path != NULL && *self->path != '\0'));
}

static void
show_status (XdFilePane *self,
             const char *title,
             const char *description)
{
  adw_status_page_set_title (self->status, title);
  adw_status_page_set_description (self->status, description);
  gtk_stack_set_visible_child_name (self->stack, "status");
}

static void
show_error (XdFilePane *self,
            GError     *error)
{
  if (g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    return;

  show_status (self, "Could Not Open Files",
               error != NULL ? error->message : "The directory could not be read.");
}

static void
add_entry_row (XdFilePane    *self,
               const char    *name,
               gboolean       directory)
{
  GtkWidget *row = gtk_list_box_row_new ();
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 10);
  GtkWidget *icon = gtk_image_new_from_icon_name (
    directory ? "folder-symbolic" : "text-x-generic-symbolic");
  GtkWidget *label = gtk_label_new (name);

  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_label_set_ellipsize (GTK_LABEL (label), PANGO_ELLIPSIZE_MIDDLE);
  gtk_widget_set_hexpand (label, TRUE);
  gtk_widget_add_css_class (icon, "dim-label");

  gtk_box_append (GTK_BOX (box), icon);
  gtk_box_append (GTK_BOX (box), label);
  if (directory)
    gtk_box_append (
      GTK_BOX (box),
      gtk_image_new_from_icon_name ("go-next-symbolic"));

  gtk_widget_set_margin_start (box, 10);
  gtk_widget_set_margin_end (box, 10);
  gtk_widget_set_margin_top (box, 7);
  gtk_widget_set_margin_bottom (box, 7);
  gtk_list_box_row_set_child (GTK_LIST_BOX_ROW (row), box);
  g_object_set_data_full (G_OBJECT (row), "path-name",
                          g_strdup (name), g_free);
  g_object_set_data (G_OBJECT (row), "directory",
                     GINT_TO_POINTER (directory));
  gtk_list_box_append (self->entries, row);
}

static void
fill_entries (XdFilePane *self,
              const char *path,
              GPtrArray  *entries)
{
  gtk_list_box_remove_all (self->entries);

  g_ptr_array_sort_values (entries, compare_entries);
  for (guint i = 0; i < entries->len; i++)
    {
      FileEntry *entry = g_ptr_array_index (entries, i);

      add_entry_row (self, entry->name, entry->directory);
    }

  g_free (self->path);
  self->path = g_strdup (path != NULL ? path : "");
  self->showing_preview = FALSE;
  set_header_path (self, self->path);

  if (entries->len == 0)
    show_status (self, "Empty Folder", "This folder has no visible files.");
  else
    gtk_stack_set_visible_child_name (self->stack, "entries");
}

static void
show_preview_text (XdFilePane *self,
                   const char *path,
                   const char *text,
                   gssize      length)
{
  gtk_text_buffer_set_text (self->preview, text, length);
  self->showing_preview = TRUE;
  set_header_path (self, path);
  gtk_stack_set_visible_child_name (self->stack, "preview");
}

/* --- local reads ---------------------------------------------------------- */

static void
on_local_listed (GObject      *source,
                 GAsyncResult *result,
                 gpointer      user_data)
{
  FileRequest *request = user_data;
  XdFilePane *self = request->pane;
  g_autoptr (GFileEnumerator) enumerator = NULL;
  g_autoptr (GPtrArray) entries =
    g_ptr_array_new_with_free_func ((GDestroyNotify) file_entry_free);
  g_autoptr (GError) error = NULL;

  enumerator =
    g_file_enumerate_children_finish (G_FILE (source), result, &error);
  if (enumerator == NULL)
    {
      show_error (self, error);
      file_request_free (request);
      return;
    }

  for (;;)
    {
      g_autoptr (GFileInfo) info =
        g_file_enumerator_next_file (enumerator, NULL, NULL);
      FileEntry *entry;

      if (info == NULL)
        break;
      if (g_file_info_get_is_hidden (info))
        continue;

      entry = g_new0 (FileEntry, 1);
      entry->name = g_strdup (g_file_info_get_name (info));
      entry->directory =
        g_file_info_get_file_type (info) == G_FILE_TYPE_DIRECTORY;
      g_ptr_array_add (entries, entry);
    }

  fill_entries (self, request->path, entries);
  file_request_free (request);
}

static void
on_local_file_read (GObject      *source,
                    GAsyncResult *result,
                    gpointer      user_data)
{
  FileRequest *request = user_data;
  XdFilePane *self = request->pane;
  g_autoptr (GBytes) bytes = NULL;
  g_autoptr (GError) error = NULL;
  gconstpointer data;
  gsize length;

  bytes = g_input_stream_read_bytes_finish (
    G_INPUT_STREAM (source), result, &error);
  if (bytes == NULL)
    {
      show_error (self, error);
      file_request_free (request);
      return;
    }

  data = g_bytes_get_data (bytes, &length);
  if (length > FILE_PREVIEW_LIMIT)
    {
      show_status (self, "File Too Large",
                   "Files larger than 1 MB are not previewed.");
    }
  else if (memchr (data, '\0', length) != NULL ||
           !g_utf8_validate (data, length, NULL))
    {
      show_status (self, "Binary File",
                   "Binary files cannot be previewed as text.");
    }
  else
    {
      show_preview_text (self, request->path, data, (gssize) length);
    }

  file_request_free (request);
}

static void
on_local_file_opened (GObject      *source,
                      GAsyncResult *result,
                      gpointer      user_data)
{
  FileRequest *request = user_data;
  g_autoptr (GError) error = NULL;

  request->stream = g_file_read_finish (G_FILE (source), result, &error);
  if (request->stream == NULL)
    {
      show_error (request->pane, error);
      file_request_free (request);
      return;
    }

  g_input_stream_read_bytes_async (
    G_INPUT_STREAM (request->stream), FILE_PREVIEW_LIMIT + 1,
    G_PRIORITY_DEFAULT, request->pane->cancellable,
    on_local_file_read, request);
}

/* --- remote reads --------------------------------------------------------- */

static void
call_remote (XdFilePane         *self,
             const char         *action,
             const char         *path,
             GAsyncReadyCallback callback,
             FileRequest        *request)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) root = NULL;

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "file-browse");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, self->chat_id);
  json_builder_set_member_name (builder, "action");
  json_builder_add_string_value (builder, action);
  json_builder_set_member_name (builder, "path");
  json_builder_add_string_value (builder, path != NULL ? path : "");
  json_builder_end_object (builder);

  root = json_builder_get_root (builder);
  xd_remote_client_call_async (self->remote, root, self->cancellable,
                               callback, request);
}

static JsonObject *
finish_remote (GObject      *source,
               GAsyncResult *result,
               FileRequest  *request)
{
  g_autoptr (GError) error = NULL;
  JsonObject *reply = xd_remote_client_call_finish (
    XD_REMOTE_CLIENT (source), result, &error);

  if (reply == NULL)
    {
      if (error != NULL &&
          g_strcmp0 (error->message, "Unknown op") == 0)
        show_status (request->pane, "Update xd on the Remote Machine",
                     "The remote server does not support file browsing.");
      else
        show_error (request->pane, error);
    }

  return reply;
}

static void
on_remote_listed (GObject      *source,
                  GAsyncResult *result,
                  gpointer      user_data)
{
  FileRequest *request = user_data;
  g_autoptr (JsonObject) reply = finish_remote (source, result, request);
  g_autoptr (GPtrArray) entries =
    g_ptr_array_new_with_free_func ((GDestroyNotify) file_entry_free);
  JsonArray *array;

  if (reply == NULL)
    {
      file_request_free (request);
      return;
    }

  array = json_object_get_array_member (reply, "entries");
  if (array == NULL)
    {
      show_status (request->pane, "Could Not Open Files",
                   "The remote server returned an invalid file list.");
      file_request_free (request);
      return;
    }

  for (guint i = 0; i < json_array_get_length (array); i++)
    {
      JsonObject *item = json_array_get_object_element (array, i);
      const char *name;
      FileEntry *entry;

      if (item == NULL)
        continue;
      name = json_object_get_string_member_with_default (item, "name", NULL);
      if (name == NULL || *name == '\0')
        continue;

      entry = g_new0 (FileEntry, 1);
      entry->name = g_strdup (name);
      entry->directory =
        json_object_get_boolean_member_with_default (item, "directory", FALSE);
      g_ptr_array_add (entries, entry);
    }

  fill_entries (request->pane, request->path, entries);
  file_request_free (request);
}

static void
on_remote_file_read (GObject      *source,
                     GAsyncResult *result,
                     gpointer      user_data)
{
  FileRequest *request = user_data;
  g_autoptr (JsonObject) reply = finish_remote (source, result, request);
  const char *content;

  if (reply == NULL)
    {
      file_request_free (request);
      return;
    }

  content =
    json_object_get_string_member_with_default (reply, "content", NULL);
  if (content == NULL)
    show_status (request->pane, "Could Not Open File",
                 "The remote server returned an invalid file.");
  else
    show_preview_text (request->pane, request->path, content, -1);

  file_request_free (request);
}

/* --- navigation ----------------------------------------------------------- */

static void
show_directory (XdFilePane *self,
                const char *path)
{
  g_autofree char *requested = g_strdup (path != NULL ? path : "");
  FileRequest *request;

  if (self->workdir == NULL || *self->workdir == '\0')
    {
      show_status (self, "No Working Directory",
                   "This chat has no files to browse.");
      return;
    }

  reset_request (self);
  self->showing_preview = FALSE;
  g_free (self->path);
  self->path = g_strdup (requested);
  set_header_path (self, self->path);
  show_status (self, "Loading…", NULL);
  request = file_request_new (self, requested);

  if (self->remote != NULL)
    {
      call_remote (self, "list", request->path, on_remote_listed, request);
      return;
    }

  {
    g_autofree char *full = local_path (self, request->path);
    g_autoptr (GFile) file = g_file_new_for_path (full);

    g_file_enumerate_children_async (
      file,
      G_FILE_ATTRIBUTE_STANDARD_NAME ","
      G_FILE_ATTRIBUTE_STANDARD_TYPE ","
      G_FILE_ATTRIBUTE_STANDARD_IS_HIDDEN,
      G_FILE_QUERY_INFO_NONE, G_PRIORITY_DEFAULT, self->cancellable,
      on_local_listed, request);
  }
}

static void
show_file (XdFilePane *self,
           const char *path)
{
  FileRequest *request;

  reset_request (self);
  self->showing_preview = TRUE;
  set_header_path (self, path);
  show_status (self, "Loading…", NULL);
  request = file_request_new (self, path);

  if (self->remote != NULL)
    {
      call_remote (self, "read", request->path,
                   on_remote_file_read, request);
      return;
    }

  {
    g_autofree char *full = local_path (self, request->path);
    g_autoptr (GFile) file = g_file_new_for_path (full);

    g_file_read_async (file, G_PRIORITY_DEFAULT, self->cancellable,
                       on_local_file_opened, request);
  }
}

static void
on_row_activated (GtkListBox    *box,
                  GtkListBoxRow *row,
                  gpointer       user_data)
{
  XdFilePane *self = user_data;
  const char *name = g_object_get_data (G_OBJECT (row), "path-name");
  g_autofree char *path = child_path (self->path, name);

  if (GPOINTER_TO_INT (
        g_object_get_data (G_OBJECT (row), "directory")))
    show_directory (self, path);
  else
    show_file (self, path);
}

static void
on_back_clicked (GtkButton *button,
                 gpointer   user_data)
{
  XdFilePane *self = user_data;

  if (self->showing_preview)
    {
      self->showing_preview = FALSE;
      set_header_path (self, self->path);
      gtk_stack_set_visible_child_name (self->stack, "entries");
      return;
    }

  {
    g_autofree char *parent = parent_path (self->path);

    show_directory (self, parent);
  }
}

void
xd_file_pane_refresh (XdFilePane *self)
{
  g_return_if_fail (XD_IS_FILE_PANE (self));

  if (!gtk_widget_is_visible (GTK_WIDGET (self)))
    return;

  show_directory (self, self->path);
}

void
xd_file_pane_set_workdir (XdFilePane *self,
                          const char *workdir)
{
  g_return_if_fail (XD_IS_FILE_PANE (self));

  if (g_strcmp0 (self->workdir, workdir) == 0)
    return;

  g_free (self->workdir);
  self->workdir = g_strdup (workdir);
  g_clear_pointer (&self->path, g_free);
  self->path = g_strdup ("");
  xd_file_pane_refresh (self);
}

void
xd_file_pane_set_remote (XdFilePane     *self,
                         XdRemoteClient *client,
                         const char     *chat_id)
{
  g_return_if_fail (XD_IS_FILE_PANE (self));
  g_return_if_fail (client == NULL || XD_IS_REMOTE_CLIENT (client));
  g_return_if_fail ((client == NULL) == (chat_id == NULL));

  if (self->remote == client && g_strcmp0 (self->chat_id, chat_id) == 0)
    return;

  reset_request (self);
  g_set_object (&self->remote, client);
  g_free (self->chat_id);
  self->chat_id = g_strdup (chat_id);
  g_clear_pointer (&self->path, g_free);
  self->path = g_strdup ("");
  xd_file_pane_refresh (self);
}

XdFilePane *
xd_file_pane_new (void)
{
  return g_object_new (XD_TYPE_FILE_PANE, NULL);
}

static void
xd_file_pane_dispose (GObject *object)
{
  XdFilePane *self = XD_FILE_PANE (object);

  g_cancellable_cancel (self->cancellable);
  g_clear_object (&self->cancellable);
  g_clear_object (&self->remote);
  g_clear_pointer (&self->workdir, g_free);
  g_clear_pointer (&self->path, g_free);
  g_clear_pointer (&self->chat_id, g_free);

  G_OBJECT_CLASS (xd_file_pane_parent_class)->dispose (object);
}

static void
xd_file_pane_class_init (XdFilePaneClass *klass)
{
  G_OBJECT_CLASS (klass)->dispose = xd_file_pane_dispose;
}

static void
xd_file_pane_init (XdFilePane *self)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *header = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
  GtkWidget *refresh =
    gtk_button_new_from_icon_name ("view-refresh-symbolic");
  GtkWidget *entries_window = gtk_scrolled_window_new ();
  GtkWidget *preview_window = gtk_scrolled_window_new ();
  GtkWidget *preview_view = gtk_text_view_new ();

  self->cancellable = g_cancellable_new ();
  self->path = g_strdup ("");

  self->back = GTK_BUTTON (
    gtk_button_new_from_icon_name ("go-previous-symbolic"));
  gtk_widget_add_css_class (GTK_WIDGET (self->back), "flat");
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->back), "Back");
  gtk_widget_set_sensitive (GTK_WIDGET (self->back), FALSE);
  g_signal_connect (self->back, "clicked",
                    G_CALLBACK (on_back_clicked), self);

  self->path_label = GTK_LABEL (gtk_label_new ("Files"));
  gtk_label_set_xalign (self->path_label, 0.0f);
  gtk_label_set_ellipsize (self->path_label, PANGO_ELLIPSIZE_START);
  gtk_widget_set_hexpand (GTK_WIDGET (self->path_label), TRUE);
  gtk_widget_add_css_class (GTK_WIDGET (self->path_label), "heading");

  gtk_widget_add_css_class (refresh, "flat");
  gtk_widget_set_tooltip_text (refresh, "Read again");
  g_signal_connect_swapped (refresh, "clicked",
                            G_CALLBACK (xd_file_pane_refresh), self);

  gtk_box_append (GTK_BOX (header), GTK_WIDGET (self->back));
  gtk_box_append (GTK_BOX (header), GTK_WIDGET (self->path_label));
  gtk_box_append (GTK_BOX (header), refresh);
  gtk_widget_set_margin_start (header, 6);
  gtk_widget_set_margin_end (header, 6);
  gtk_widget_set_margin_top (header, 6);
  gtk_widget_set_margin_bottom (header, 6);

  self->entries = GTK_LIST_BOX (gtk_list_box_new ());
  gtk_list_box_set_selection_mode (self->entries, GTK_SELECTION_SINGLE);
  gtk_widget_add_css_class (GTK_WIDGET (self->entries), "xd-file-list");
  g_signal_connect (self->entries, "row-activated",
                    G_CALLBACK (on_row_activated), self);
  gtk_scrolled_window_set_child (
    GTK_SCROLLED_WINDOW (entries_window), GTK_WIDGET (self->entries));
  gtk_scrolled_window_set_policy (
    GTK_SCROLLED_WINDOW (entries_window),
    GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);

  gtk_text_view_set_editable (GTK_TEXT_VIEW (preview_view), FALSE);
  gtk_text_view_set_cursor_visible (GTK_TEXT_VIEW (preview_view), FALSE);
  gtk_text_view_set_monospace (GTK_TEXT_VIEW (preview_view), TRUE);
  gtk_text_view_set_wrap_mode (GTK_TEXT_VIEW (preview_view), GTK_WRAP_NONE);
  gtk_text_view_set_left_margin (GTK_TEXT_VIEW (preview_view), 12);
  gtk_text_view_set_right_margin (GTK_TEXT_VIEW (preview_view), 12);
  gtk_text_view_set_top_margin (GTK_TEXT_VIEW (preview_view), 10);
  gtk_text_view_set_bottom_margin (GTK_TEXT_VIEW (preview_view), 10);
  gtk_widget_add_css_class (preview_view, "xd-file-preview");
  self->preview =
    gtk_text_view_get_buffer (GTK_TEXT_VIEW (preview_view));
  gtk_scrolled_window_set_child (
    GTK_SCROLLED_WINDOW (preview_window), preview_view);
  gtk_scrolled_window_set_policy (
    GTK_SCROLLED_WINDOW (preview_window),
    GTK_POLICY_AUTOMATIC, GTK_POLICY_AUTOMATIC);

  self->status = ADW_STATUS_PAGE (adw_status_page_new ());
  adw_status_page_set_icon_name (self->status, "folder-symbolic");
  adw_status_page_set_title (self->status, "Files");

  self->stack = GTK_STACK (gtk_stack_new ());
  gtk_stack_add_named (self->stack, entries_window, "entries");
  gtk_stack_add_named (self->stack, preview_window, "preview");
  gtk_stack_add_named (
    self->stack, GTK_WIDGET (self->status), "status");
  gtk_widget_set_vexpand (GTK_WIDGET (self->stack), TRUE);

  gtk_box_append (GTK_BOX (box), header);
  gtk_box_append (
    GTK_BOX (box), gtk_separator_new (GTK_ORIENTATION_HORIZONTAL));
  gtk_box_append (GTK_BOX (box), GTK_WIDGET (self->stack));
  adw_bin_set_child (ADW_BIN (self), box);
}
