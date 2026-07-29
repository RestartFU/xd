#include "dir-browser.h"
#include "panel-style.h"

/*
 * The panel's own look, loaded once.
 *
 * It travels with the widget rather than living in the application's
 * stylesheet: this is a surface with no decoration of its own, so what it is
 * made of -- the black, the corners, the shadow that lifts it off what is
 * behind -- is part of the widget rather than a theme choice made elsewhere.
 */
static const char BROWSER_STYLE[] =
  /* One surface: the list is part of the panel, not a pane inset into it. */
  ".xd-browser scrolledwindow,"
  ".xd-browser listview { background: transparent; }\n"

  ".xd-browser listview > row {"
  "  border-radius: 9px;"
  "  margin: 1px 8px;"
  "  padding: 7px 10px;"
  "}\n"
  ".xd-browser listview > row:hover { background: alpha(#ffffff, 0.05); }\n"
  ".xd-browser listview > row:selected {"
  "  background: alpha(#ffffff, 0.11);"
  "  color: inherit;"
  "}\n"

  /* The path reads as a path: what it is, rather than a title. */
  ".xd-browser-path { font-family: monospace; font-size: 96%;"
  " color: alpha(#ffffff, 0.85); }\n";

static void
ensure_style (void)
{
  static gsize once = 0;

  if (g_once_init_enter (&once))
    {
      g_autoptr (GtkCssProvider) provider = gtk_css_provider_new ();

      gtk_css_provider_load_from_string (provider, BROWSER_STYLE);
      gtk_style_context_add_provider_for_display (
        gdk_display_get_default (), GTK_STYLE_PROVIDER (provider),
        GTK_STYLE_PROVIDER_PRIORITY_APPLICATION + 1);

      g_once_init_leave (&once, 1);
    }
}

/*
 * One window, one directory at a time.
 *
 * Where the names come from is the only thing that differs between a local
 * chat and one on a daemon, and it is one function: everything else -- the
 * list, the keys, the path in the header -- has no idea which machine it is
 * looking at.
 */

typedef struct
{
  grefcount refs;
  GtkWindow *window;
  XdRemoteTree *remote;         /* NULL: this machine */

  GtkLabel *path_label;
  GtkLabel *trouble;
  GtkStringList *entries;
  GtkSingleSelection *selection;
  GtkListView *list_view;

  char *path;
  GCancellable *cancellable;

  XdDirChosenFunc chosen;
  gpointer user_data;
  gboolean answered;
} Browser;

static void show_directory (Browser *self, const char *path);

static Browser *
browser_ref (Browser *self)
{
  g_ref_count_inc (&self->refs);
  return self;
}

static void
browser_unref (Browser *self)
{
  if (!g_ref_count_dec (&self->refs))
    return;

  g_clear_object (&self->entries);
  g_clear_object (&self->selection);
  g_clear_object (&self->cancellable);
  g_clear_object (&self->remote);
  g_free (self->path);
  g_free (self);
}

static void
browser_window_gone (Browser *self)
{
  /*
   * Directory reads finish asynchronously. Destroying the window cancels
   * them, but cancellation still completes their callbacks; keep this state
   * alive until those callbacks have released it and make them ignore the
   * widgets that went away with the window.
   */
  self->window = NULL;
  self->path_label = NULL;
  self->trouble = NULL;
  self->list_view = NULL;

  /* Dismissed rather than answered: the caller is still waiting to hear
   * something, and "nothing was picked" is an answer. Do not delay it until
   * an outstanding remote request happens to return. */
  if (!self->answered)
    {
      self->answered = TRUE;
      self->chosen (NULL, self->user_data);
    }

  g_cancellable_cancel (self->cancellable);
  browser_unref (self);
}

static void
answer (Browser    *self,
        const char *path)
{
  if (!self->answered)
    {
      self->answered = TRUE;
      self->chosen (path, self->user_data);
    }

  gtk_window_destroy (self->window);
}

/* --- reading a directory ---------------------------------------------------- */

static void
fill (Browser            *self,
      const char         *path,
      const char *const  *names)
{
  guint n = g_list_model_get_n_items (G_LIST_MODEL (self->entries));

  gtk_widget_set_visible (GTK_WIDGET (self->trouble), FALSE);

  gtk_string_list_splice (self->entries, 0, n, names);

  g_free (self->path);
  self->path = g_strdup (path);
  gtk_label_set_label (self->path_label, path);

  if (names != NULL && names[0] != NULL)
    gtk_single_selection_set_selected (self->selection, 0);
}

static void
on_remote_listed (const char        *path,
                  const char *const *entries,
                  const char        *trouble,
                  gpointer           user_data)
{
  Browser *self = user_data;

  if (self->window == NULL)
    {
      browser_unref (self);
      return;
    }

  if (path != NULL)
    {
      fill (self, path, entries);
      browser_unref (self);
      return;
    }

  /*
   * Said rather than swallowed.
   *
   * The likeliest reason is a daemon older than the client asking -- it has no
   * op for this -- and an empty list would look like an empty disk.
   */
  gtk_label_set_label (self->trouble, trouble);
  gtk_widget_set_visible (GTK_WIDGET (self->trouble), TRUE);
  browser_unref (self);
}

static void
on_local_listed (GObject      *source,
                 GAsyncResult *result,
                 gpointer      user_data)
{
  Browser *self = user_data;
  g_autoptr (GFileEnumerator) enumerator = NULL;
  g_autoptr (GPtrArray) names = g_ptr_array_new_with_free_func (g_free);
  g_autoptr (GError) error = NULL;
  g_autofree char *path = NULL;

  enumerator = g_file_enumerate_children_finish (G_FILE (source), result, &error);
  if (enumerator == NULL || self->window == NULL)
    {
      browser_unref (self);
      return;
    }

  for (;;)
    {
      GFileInfo *info = g_file_enumerator_next_file (enumerator, NULL, NULL);

      if (info == NULL)
        break;

      if (g_file_info_get_file_type (info) == G_FILE_TYPE_DIRECTORY &&
          !g_file_info_get_is_hidden (info))
        g_ptr_array_add (names, g_strdup (g_file_info_get_name (info)));

      g_object_unref (info);
    }

  g_ptr_array_sort_values (names, (GCompareFunc) g_strcmp0);
  g_ptr_array_add (names, NULL);

  path = g_file_get_path (G_FILE (source));
  fill (self, path, (const char *const *) names->pdata);
  browser_unref (self);
}

static void
show_directory (Browser    *self,
                const char *path)
{
  if (self->remote != NULL)
    {
      xd_remote_tree_list_dir (self->remote, path, self->cancellable,
                               on_remote_listed, browser_ref (self));
      return;
    }

  {
    g_autoptr (GFile) file = path != NULL ? g_file_new_for_path (path)
                                          : g_file_new_for_path (g_get_home_dir ());

    g_file_enumerate_children_async (file,
                                     G_FILE_ATTRIBUTE_STANDARD_NAME ","
                                     G_FILE_ATTRIBUTE_STANDARD_TYPE ","
                                     G_FILE_ATTRIBUTE_STANDARD_IS_HIDDEN,
                                     G_FILE_QUERY_INFO_NONE, G_PRIORITY_DEFAULT,
                                     self->cancellable, on_local_listed,
                                     browser_ref (self));
  }
}

/* --- moving about ----------------------------------------------------------- */

static char *
selected_path (Browser *self)
{
  GtkStringObject *item = gtk_single_selection_get_selected_item (self->selection);

  if (item == NULL || self->path == NULL)
    return NULL;

  return g_build_filename (self->path, gtk_string_object_get_string (item), NULL);
}

static void
descend (Browser *self)
{
  g_autofree char *into = selected_path (self);

  if (into != NULL)
    show_directory (self, into);
}

static void
ascend (Browser *self)
{
  g_autofree char *up = NULL;

  if (self->path == NULL)
    return;

  up = g_path_get_dirname (self->path);

  /* The filesystem root is its own parent; stopping there is the top. */
  if (g_strcmp0 (up, self->path) != 0)
    show_directory (self, up);
}

static gboolean
on_key (GtkEventControllerKey *controller,
        guint                  keyval,
        guint                  keycode,
        GdkModifierType        state,
        gpointer               user_data)
{
  Browser *self = user_data;

  switch (keyval)
    {
    case GDK_KEY_Escape:
      answer (self, NULL);
      return GDK_EVENT_STOP;

    case GDK_KEY_BackSpace:
    case GDK_KEY_Left:
      ascend (self);
      return GDK_EVENT_STOP;

    case GDK_KEY_Return:
    case GDK_KEY_KP_Enter:
    case GDK_KEY_Right:
      /* Ctrl is "this one, the one I am looking at" rather than "further in". */
      if ((state & GDK_CONTROL_MASK) != 0)
        answer (self, self->path);
      else
        descend (self);
      return GDK_EVENT_STOP;

    default:
      return GDK_EVENT_PROPAGATE;
    }
}

static void
on_row_activated (GtkListView *list_view,
                  guint        position,
                  gpointer     user_data)
{
  descend (user_data);
}

static void
on_use_clicked (GtkButton *button,
                gpointer   user_data)
{
  Browser *self = user_data;

  answer (self, self->path);
}

/* --- the window ------------------------------------------------------------- */

static void
on_item_setup (GtkSignalListItemFactory *factory,
               GtkListItem              *item,
               gpointer                  user_data)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 12);
  GtkWidget *icon = gtk_image_new_from_icon_name ("folder-symbolic");
  GtkWidget *label = gtk_label_new (NULL);

  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_label_set_ellipsize (GTK_LABEL (label), PANGO_ELLIPSIZE_MIDDLE);
  gtk_widget_set_hexpand (label, TRUE);

  gtk_widget_add_css_class (icon, "dim-label");

  gtk_box_append (GTK_BOX (box), icon);
  gtk_box_append (GTK_BOX (box), label);
  gtk_widget_set_margin_top (box, 2);
  gtk_widget_set_margin_bottom (box, 2);

  gtk_list_item_set_child (item, box);
}

static void
on_item_bind (GtkSignalListItemFactory *factory,
              GtkListItem              *item,
              gpointer                  user_data)
{
  GtkWidget *box = gtk_list_item_get_child (item);
  GtkWidget *label = gtk_widget_get_last_child (box);
  GtkStringObject *entry = gtk_list_item_get_item (item);

  gtk_label_set_label (GTK_LABEL (label), gtk_string_object_get_string (entry));
}

static GtkWidget *
hint (const char *key,
      const char *what)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
  GtkWidget *label = gtk_label_new (key);
  GtkWidget *text = gtk_label_new (what);

  gtk_widget_add_css_class (label, "xd-key");
  gtk_widget_add_css_class (text, "dim-label");
  gtk_widget_add_css_class (text, "caption");

  gtk_box_append (GTK_BOX (box), label);
  gtk_box_append (GTK_BOX (box), text);

  return box;
}

void
xd_dir_browser_present (GtkWidget       *parent,
                        XdRemoteTree    *remote,
                        const char      *start,
                        XdDirChosenFunc  chosen,
                        gpointer         user_data)
{
  Browser *self;
  GtkWidget *window;
  GtkWidget *column;
  GtkWidget *header;
  GtkWidget *use;
  GtkWidget *scrolled;
  GtkWidget *footer;
  GtkListItemFactory *factory;

  g_return_if_fail (GTK_IS_WIDGET (parent));
  g_return_if_fail (chosen != NULL);

  xd_panel_style_ensure ();
  ensure_style ();

  self = g_new0 (Browser, 1);
  g_ref_count_init (&self->refs);
  self->remote = remote != NULL ? g_object_ref (remote) : NULL;
  self->chosen = chosen;
  self->user_data = user_data;
  self->cancellable = g_cancellable_new ();
  self->entries = gtk_string_list_new (NULL);

  window = gtk_window_new ();
  self->window = GTK_WINDOW (window);

  gtk_window_set_transient_for (GTK_WINDOW (window),
                                GTK_WINDOW (gtk_widget_get_root (parent)));
  gtk_window_set_application (
    GTK_WINDOW (window),
    gtk_window_get_application (
      GTK_WINDOW (gtk_widget_get_root (parent))));
  gtk_window_set_modal (GTK_WINDOW (window), TRUE);
  gtk_window_set_decorated (GTK_WINDOW (window), FALSE);
  gtk_window_set_default_size (GTK_WINDOW (window), 620, 460);
  gtk_widget_add_css_class (window, "xd-panel");
  gtk_widget_add_css_class (window, "xd-browser");

  /* The path being looked at, and the way to take it. */
  self->path_label = GTK_LABEL (gtk_label_new (NULL));
  gtk_label_set_ellipsize (self->path_label, PANGO_ELLIPSIZE_START);
  gtk_label_set_xalign (self->path_label, 0.0f);
  gtk_widget_set_hexpand (GTK_WIDGET (self->path_label), TRUE);
  gtk_widget_add_css_class (GTK_WIDGET (self->path_label), "xd-browser-path");

  use = gtk_button_new_with_label ("Work here");
  gtk_widget_add_css_class (use, "xd-panel-action");
  g_signal_connect (use, "clicked", G_CALLBACK (on_use_clicked), self);

  header = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 10);
  gtk_box_append (GTK_BOX (header), GTK_WIDGET (self->path_label));
  gtk_box_append (GTK_BOX (header), use);
  gtk_widget_add_css_class (header, "xd-panel-bar");
  gtk_widget_add_css_class (header, "xd-panel-head");

  self->trouble = GTK_LABEL (gtk_label_new (NULL));
  gtk_label_set_wrap (self->trouble, TRUE);
  gtk_label_set_xalign (self->trouble, 0.0f);
  gtk_widget_set_visible (GTK_WIDGET (self->trouble), FALSE);
  gtk_widget_add_css_class (GTK_WIDGET (self->trouble), "error");
  gtk_widget_add_css_class (GTK_WIDGET (self->trouble), "xd-panel-bar");

  factory = gtk_signal_list_item_factory_new ();
  g_signal_connect (factory, "setup", G_CALLBACK (on_item_setup), self);
  g_signal_connect (factory, "bind", G_CALLBACK (on_item_bind), self);

  self->selection = gtk_single_selection_new (g_object_ref (G_LIST_MODEL (self->entries)));
  self->list_view = GTK_LIST_VIEW (gtk_list_view_new (
    GTK_SELECTION_MODEL (self->selection), factory));
  gtk_list_view_set_single_click_activate (self->list_view, FALSE);
  gtk_widget_add_css_class (GTK_WIDGET (self->list_view), "navigation-sidebar");
  g_signal_connect (self->list_view, "activate", G_CALLBACK (on_row_activated), self);

  scrolled = gtk_scrolled_window_new ();
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (scrolled),
                                 GTK_WIDGET (self->list_view));
  gtk_widget_set_vexpand (scrolled, TRUE);

  footer = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 16);
  gtk_box_append (GTK_BOX (footer), hint ("↑↓", "Move"));
  gtk_box_append (GTK_BOX (footer), hint ("Enter", "Open"));
  gtk_box_append (GTK_BOX (footer), hint ("Backspace", "Back"));
  gtk_box_append (GTK_BOX (footer), hint ("Esc", "Use the folder's"));
  gtk_widget_add_css_class (footer, "xd-panel-bar");
  gtk_widget_add_css_class (footer, "xd-panel-foot");

  column = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  gtk_box_append (GTK_BOX (column), header);
  gtk_box_append (GTK_BOX (column), GTK_WIDGET (self->trouble));
  gtk_box_append (GTK_BOX (column), scrolled);
  gtk_box_append (GTK_BOX (column), footer);
  gtk_window_set_child (GTK_WINDOW (window), column);

  {
    GtkEventController *keys = gtk_event_controller_key_new ();

    gtk_event_controller_set_propagation_phase (keys, GTK_PHASE_CAPTURE);
    g_signal_connect (keys, "key-pressed", G_CALLBACK (on_key), self);
    gtk_widget_add_controller (window, keys);
  }

  g_object_set_data_full (G_OBJECT (window), "browser", self,
                          (GDestroyNotify) browser_window_gone);

  show_directory (self, start);

  gtk_window_present (GTK_WINDOW (window));
  gtk_widget_grab_focus (GTK_WIDGET (self->list_view));
}
