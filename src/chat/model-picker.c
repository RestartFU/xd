#include "model-picker.h"

#include "backend/backend.h"

/* How many rows get a Ctrl+N shortcut. */
#define N_SHORTCUTS 9

typedef struct
{
  const AiBackend *backend;
  const AiModel *model;
} Entry;

/* GtkMenuButton is final, so the picker wraps one rather than deriving it. */
struct _HyModelPicker
{
  AdwBin parent_instance;

  GtkMenuButton *button;
  GSettings *settings;
  GHashTable *favorites;        /* "backend/model" keys */

  char *backend_id;
  char *model_id;

  GtkImage *button_icon;
  GtkLabel *button_label;

  GtkSearchEntry *search;
  GtkListBox *list;
  GtkBox *rail;
  GtkStack *stack;

  /* NULL means "starred only"; otherwise the backend being browsed. */
  const AiBackend *filter;
  gboolean showing_favorites;
  gboolean syncing_rail;

  GPtrArray *visible;           /* Entry*, in display order */
};

enum
{
  SIGNAL_MODEL_CHOSEN,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (HyModelPicker, hy_model_picker, ADW_TYPE_BIN)

static void rebuild_list (HyModelPicker *self);

/* --- favorites ------------------------------------------------------------ */

/* A model id may be NULL, so the key always has both halves and an empty
 * second half means the backend's default. */
static char *
favorite_key (const AiBackend *backend,
              const char      *model_id)
{
  return g_strdup_printf ("%s/%s", backend->id, model_id != NULL ? model_id : "");
}

static gboolean
is_favorite (HyModelPicker   *self,
             const AiBackend *backend,
             const char      *model_id)
{
  g_autofree char *key = favorite_key (backend, model_id);

  return g_hash_table_contains (self->favorites, key);
}

static void
save_favorites (HyModelPicker *self)
{
  g_autoptr (GPtrArray) keys = g_ptr_array_new ();
  GHashTableIter iter;
  gpointer key;

  g_hash_table_iter_init (&iter, self->favorites);
  while (g_hash_table_iter_next (&iter, &key, NULL))
    g_ptr_array_add (keys, key);
  g_ptr_array_add (keys, NULL);

  g_settings_set_strv (self->settings, "favorite-models",
                       (const char * const *) keys->pdata);
}

static void
toggle_favorite (HyModelPicker   *self,
                 const AiBackend *backend,
                 const char      *model_id)
{
  g_autofree char *key = favorite_key (backend, model_id);

  if (g_hash_table_contains (self->favorites, key))
    g_hash_table_remove (self->favorites, key);
  else
    g_hash_table_add (self->favorites, g_steal_pointer (&key));

  save_favorites (self);
  rebuild_list (self);
}

/* --- the button ----------------------------------------------------------- */

static void
update_button (HyModelPicker *self)
{
  const AiBackend *backend = ai_backend_lookup (self->backend_id);

  if (backend == NULL)
    {
      gtk_label_set_label (self->button_label, "No assistant");
      gtk_image_set_from_icon_name (self->button_icon, "dialog-warning-symbolic");
      return;
    }

  gtk_image_set_from_icon_name (self->button_icon, backend->icon_name);
  gtk_label_set_label (self->button_label,
                       ai_backend_model_label (backend, self->model_id));
}

/* --- rows ----------------------------------------------------------------- */

static void
on_star_toggled (GtkButton *button,
                 gpointer   user_data)
{
  HyModelPicker *self = user_data;
  const Entry *entry = g_object_get_data (G_OBJECT (button), "entry");

  toggle_favorite (self, entry->backend, entry->model->id);
}

static GtkWidget *
build_row (HyModelPicker *self,
           const Entry   *entry,
           int            index)
{
  GtkWidget *row = gtk_list_box_row_new ();
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 10);
  GtkWidget *icon = gtk_image_new_from_icon_name (entry->backend->icon_name);
  GtkWidget *names = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *name = gtk_label_new (entry->model->display_name);
  GtkWidget *provider = gtk_label_new (entry->backend->display_name);
  GtkWidget *star = gtk_button_new_from_icon_name (
    is_favorite (self, entry->backend, entry->model->id) ? "starred-symbolic"
                                                         : "non-starred-symbolic");

  gtk_label_set_xalign (GTK_LABEL (name), 0.0f);
  gtk_label_set_xalign (GTK_LABEL (provider), 0.0f);
  gtk_widget_add_css_class (provider, "dim-label");
  gtk_widget_add_css_class (provider, "caption");
  gtk_box_append (GTK_BOX (names), name);
  gtk_box_append (GTK_BOX (names), provider);
  gtk_widget_set_hexpand (names, TRUE);

  gtk_box_append (GTK_BOX (box), icon);
  gtk_box_append (GTK_BOX (box), names);

  /* Only the first few rows get a shortcut; showing one on row 30 would be a
   * promise the keyboard cannot keep. */
  if (index < N_SHORTCUTS)
    {
      g_autofree char *accel = g_strdup_printf ("Ctrl+%d", index + 1);
      GtkWidget *hint = gtk_label_new (accel);

      gtk_widget_add_css_class (hint, "dim-label");
      gtk_widget_add_css_class (hint, "caption");
      gtk_widget_set_valign (hint, GTK_ALIGN_CENTER);
      gtk_box_append (GTK_BOX (box), hint);
    }

  gtk_widget_add_css_class (star, "flat");
  gtk_widget_set_valign (star, GTK_ALIGN_CENTER);
  gtk_widget_set_tooltip_text (star, "Star this model");
  g_object_set_data (G_OBJECT (star), "entry", (gpointer) entry);
  g_signal_connect (star, "clicked", G_CALLBACK (on_star_toggled), self);
  gtk_box_append (GTK_BOX (box), star);

  gtk_widget_set_margin_top (box, 6);
  gtk_widget_set_margin_bottom (box, 6);
  gtk_widget_set_margin_start (box, 10);
  gtk_widget_set_margin_end (box, 6);

  gtk_list_box_row_set_child (GTK_LIST_BOX_ROW (row), box);

  return row;
}

/* --- list ----------------------------------------------------------------- */

static gboolean
matches_search (const Entry *entry,
                const char  *needle)
{
  g_autofree char *haystack = NULL;

  if (needle == NULL || *needle == '\0')
    return TRUE;

  haystack = g_utf8_strdown (entry->model->display_name, -1);
  if (strstr (haystack, needle) != NULL)
    return TRUE;

  g_free (haystack);
  haystack = g_utf8_strdown (entry->backend->display_name, -1);

  return strstr (haystack, needle) != NULL;
}

static void
rebuild_list (HyModelPicker *self)
{
  g_autofree char *needle = NULL;
  const AiBackend *const *backends;
  const char *text;
  GtkWidget *child;
  guint n_backends;

  while ((child = gtk_widget_get_first_child (GTK_WIDGET (self->list))) != NULL)
    gtk_list_box_remove (self->list, child);

  g_ptr_array_set_size (self->visible, 0);

  text = gtk_editable_get_text (GTK_EDITABLE (self->search));
  needle = g_utf8_strdown (text, -1);

  backends = ai_backend_all (&n_backends);
  for (guint b = 0; b < n_backends; b++)
    {
      const AiBackend *backend = backends[b];

      if (!self->showing_favorites && self->filter != backend)
        continue;

      for (gsize m = 0; m < backend->n_models; m++)
        {
          Entry *entry;

          if (self->showing_favorites &&
              !is_favorite (self, backend, backend->models[m].id))
            continue;

          entry = g_new0 (Entry, 1);
          entry->backend = backend;
          entry->model = &backend->models[m];

          if (!matches_search (entry, needle))
            {
              g_free (entry);
              continue;
            }

          g_ptr_array_add (self->visible, entry);
        }
    }

  for (guint i = 0; i < self->visible->len; i++)
    gtk_list_box_append (self->list, build_row (self, g_ptr_array_index (self->visible, i), (int) i));

  gtk_stack_set_visible_child_name (self->stack,
                                    self->visible->len > 0 ? "list" : "empty");
}

static void
choose_entry (HyModelPicker *self,
              const Entry   *entry)
{
  g_free (self->backend_id);
  g_free (self->model_id);
  self->backend_id = g_strdup (entry->backend->id);
  self->model_id = g_strdup (entry->model->id);

  update_button (self);
  gtk_menu_button_popdown (self->button);

  g_signal_emit (self, signals[SIGNAL_MODEL_CHOSEN], 0,
                 self->backend_id, self->model_id);
}

static void
on_row_activated (GtkListBox    *list,
                  GtkListBoxRow *row,
                  gpointer       user_data)
{
  HyModelPicker *self = user_data;
  int index = gtk_list_box_row_get_index (row);

  if (index >= 0 && (guint) index < self->visible->len)
    choose_entry (self, g_ptr_array_index (self->visible, index));
}

static void
on_search_changed (GtkSearchEntry *search,
                   gpointer        user_data)
{
  rebuild_list (user_data);
}

/* Ctrl+1 through Ctrl+9 pick the corresponding visible row. */
static void
on_choose_action (GtkWidget  *widget,
                  const char *action_name,
                  GVariant   *parameter)
{
  HyModelPicker *self = HY_MODEL_PICKER (widget);
  gint32 index = g_variant_get_int32 (parameter);

  if (index >= 0 && (guint) index < self->visible->len)
    choose_entry (self, g_ptr_array_index (self->visible, index));
}

/* --- the provider rail ---------------------------------------------------- */

static void
on_rail_toggled (GtkToggleButton *button,
                 gpointer         user_data)
{
  HyModelPicker *self = user_data;
  const AiBackend *backend;

  if (self->syncing_rail || !gtk_toggle_button_get_active (button))
    return;

  backend = g_object_get_data (G_OBJECT (button), "backend");
  self->showing_favorites = backend == NULL;
  self->filter = backend;

  rebuild_list (self);
}

static void
sync_rail (HyModelPicker *self)
{
  GtkWidget *child;

  self->syncing_rail = TRUE;

  for (child = gtk_widget_get_first_child (GTK_WIDGET (self->rail));
       child != NULL;
       child = gtk_widget_get_next_sibling (child))
    {
      const AiBackend *backend = g_object_get_data (G_OBJECT (child), "backend");
      gboolean active = self->showing_favorites ? backend == NULL
                                                : backend == self->filter;

      gtk_toggle_button_set_active (GTK_TOGGLE_BUTTON (child), active);
    }

  self->syncing_rail = FALSE;
}

static GtkWidget *
build_rail_button (HyModelPicker   *self,
                   const AiBackend *backend,
                   GtkWidget       *group)
{
  GtkWidget *button = gtk_toggle_button_new ();

  gtk_button_set_icon_name (GTK_BUTTON (button),
                            backend != NULL ? backend->icon_name : "starred-symbolic");
  gtk_widget_set_tooltip_text (button,
                               backend != NULL ? backend->display_name : "Starred");
  gtk_widget_add_css_class (button, "flat");
  g_object_set_data (G_OBJECT (button), "backend", (gpointer) backend);

  if (group != NULL)
    gtk_toggle_button_set_group (GTK_TOGGLE_BUTTON (button), GTK_TOGGLE_BUTTON (group));

  g_signal_connect (button, "toggled", G_CALLBACK (on_rail_toggled), self);
  gtk_box_append (self->rail, button);

  return button;
}

/* --- public API ----------------------------------------------------------- */

void
hy_model_picker_set_selected (HyModelPicker *self,
                              const char    *backend_id,
                              const char    *model_id)
{
  g_return_if_fail (HY_IS_MODEL_PICKER (self));

  g_free (self->backend_id);
  g_free (self->model_id);
  self->backend_id = g_strdup (backend_id);
  self->model_id = g_strdup (model_id);

  /* Open the picker on the chat's own assistant rather than wherever it was
   * left last time. */
  self->filter = ai_backend_lookup (backend_id);
  self->showing_favorites = FALSE;

  update_button (self);
  sync_rail (self);
  rebuild_list (self);
}

HyModelPicker *
hy_model_picker_new (void)
{
  return g_object_new (HY_TYPE_MODEL_PICKER, NULL);
}

/* --- construction --------------------------------------------------------- */

static GtkWidget *
build_popover_content (HyModelPicker *self)
{
  GtkWidget *columns = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);
  GtkWidget *right = gtk_box_new (GTK_ORIENTATION_VERTICAL, 6);
  GtkWidget *scroller = gtk_scrolled_window_new ();
  GtkWidget *empty = adw_status_page_new ();
  const AiBackend *const *backends;
  GtkWidget *group = NULL;
  guint n_backends;

  self->rail = GTK_BOX (gtk_box_new (GTK_ORIENTATION_VERTICAL, 4));
  gtk_widget_set_margin_top (GTK_WIDGET (self->rail), 6);
  gtk_widget_set_margin_bottom (GTK_WIDGET (self->rail), 6);
  gtk_widget_set_margin_start (GTK_WIDGET (self->rail), 6);
  gtk_widget_set_margin_end (GTK_WIDGET (self->rail), 6);

  group = build_rail_button (self, NULL, NULL);
  backends = ai_backend_all (&n_backends);
  for (guint i = 0; i < n_backends; i++)
    build_rail_button (self, backends[i], group);

  self->search = GTK_SEARCH_ENTRY (gtk_search_entry_new ());
  gtk_search_entry_set_placeholder_text (self->search, "Search models…");
  gtk_widget_set_margin_top (GTK_WIDGET (self->search), 6);
  gtk_widget_set_margin_start (GTK_WIDGET (self->search), 6);
  gtk_widget_set_margin_end (GTK_WIDGET (self->search), 6);
  g_signal_connect (self->search, "search-changed",
                    G_CALLBACK (on_search_changed), self);

  self->list = GTK_LIST_BOX (gtk_list_box_new ());
  gtk_list_box_set_selection_mode (self->list, GTK_SELECTION_NONE);
  g_signal_connect (self->list, "row-activated", G_CALLBACK (on_row_activated), self);

  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scroller),
                                  GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (scroller),
                                 GTK_WIDGET (self->list));
  gtk_widget_set_vexpand (scroller, TRUE);

  adw_status_page_set_icon_name (ADW_STATUS_PAGE (empty), "system-search-symbolic");
  adw_status_page_set_title (ADW_STATUS_PAGE (empty), "No Models");

  self->stack = GTK_STACK (gtk_stack_new ());
  gtk_stack_add_named (self->stack, scroller, "list");
  gtk_stack_add_named (self->stack, empty, "empty");
  gtk_widget_set_vexpand (GTK_WIDGET (self->stack), TRUE);

  gtk_box_append (GTK_BOX (right), GTK_WIDGET (self->search));
  gtk_box_append (GTK_BOX (right), GTK_WIDGET (self->stack));
  gtk_widget_set_hexpand (right, TRUE);

  gtk_box_append (GTK_BOX (columns), GTK_WIDGET (self->rail));
  gtk_box_append (GTK_BOX (columns), gtk_separator_new (GTK_ORIENTATION_VERTICAL));
  gtk_box_append (GTK_BOX (columns), right);

  gtk_widget_set_size_request (columns, 380, 360);

  return columns;
}

static void
on_popover_shown (GtkPopover *popover,
                  gpointer    user_data)
{
  HyModelPicker *self = user_data;

  gtk_editable_set_text (GTK_EDITABLE (self->search), "");
  rebuild_list (self);
  gtk_widget_grab_focus (GTK_WIDGET (self->search));
}

static void
hy_model_picker_dispose (GObject *object)
{
  HyModelPicker *self = HY_MODEL_PICKER (object);

  g_clear_pointer (&self->visible, g_ptr_array_unref);
  g_clear_pointer (&self->favorites, g_hash_table_unref);
  g_clear_object (&self->settings);

  G_OBJECT_CLASS (hy_model_picker_parent_class)->dispose (object);
}

static void
hy_model_picker_finalize (GObject *object)
{
  HyModelPicker *self = HY_MODEL_PICKER (object);

  g_clear_pointer (&self->backend_id, g_free);
  g_clear_pointer (&self->model_id, g_free);

  G_OBJECT_CLASS (hy_model_picker_parent_class)->finalize (object);
}

static void
hy_model_picker_class_init (HyModelPickerClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);
  GtkWidgetClass *widget_class = GTK_WIDGET_CLASS (klass);

  object_class->dispose = hy_model_picker_dispose;
  object_class->finalize = hy_model_picker_finalize;

  signals[SIGNAL_MODEL_CHOSEN] =
    g_signal_new ("model-chosen", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 2,
                  G_TYPE_STRING, G_TYPE_STRING);

  gtk_widget_class_install_action (widget_class, "picker.choose", "i",
                                   on_choose_action);

  for (int i = 0; i < N_SHORTCUTS; i++)
    gtk_widget_class_add_binding_action (widget_class, GDK_KEY_1 + i,
                                         GDK_CONTROL_MASK, "picker.choose",
                                         "i", i);
}

static void
hy_model_picker_init (HyModelPicker *self)
{
  GtkWidget *content = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
  GtkWidget *popover = gtk_popover_new ();
  g_auto (GStrv) favorites = NULL;

  self->settings = g_settings_new (HY_APP_ID);
  self->favorites = g_hash_table_new_full (g_str_hash, g_str_equal, g_free, NULL);
  self->visible = g_ptr_array_new_with_free_func (g_free);

  favorites = g_settings_get_strv (self->settings, "favorite-models");
  for (gsize i = 0; favorites[i] != NULL; i++)
    g_hash_table_add (self->favorites, g_strdup (favorites[i]));

  self->button_icon = GTK_IMAGE (gtk_image_new ());
  self->button_label = GTK_LABEL (gtk_label_new (NULL));
  gtk_label_set_ellipsize (self->button_label, PANGO_ELLIPSIZE_END);

  gtk_box_append (GTK_BOX (content), GTK_WIDGET (self->button_icon));
  gtk_box_append (GTK_BOX (content), GTK_WIDGET (self->button_label));
  gtk_box_append (GTK_BOX (content), gtk_image_new_from_icon_name ("pan-down-symbolic"));

  self->button = GTK_MENU_BUTTON (gtk_menu_button_new ());
  gtk_menu_button_set_child (self->button, content);
  gtk_widget_add_css_class (GTK_WIDGET (self->button), "flat");

  gtk_popover_set_child (GTK_POPOVER (popover), build_popover_content (self));
  gtk_popover_set_has_arrow (GTK_POPOVER (popover), FALSE);
  g_signal_connect (popover, "show", G_CALLBACK (on_popover_shown), self);
  gtk_menu_button_set_popover (self->button, popover);

  adw_bin_set_child (ADW_BIN (self), GTK_WIDGET (self->button));

  update_button (self);
}
