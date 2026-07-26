#include "option-picker.h"

typedef struct
{
  char *label;
  char *description;
  GtkLabel *row_label;
  GtkImage *check;
  GtkListBoxRow *row;
} Choice;

struct _XdOptionPicker
{
  AdwBin parent_instance;

  GtkMenuButton *button;
  GtkLabel *button_label;
  GtkListBox *list;
  GPtrArray *choices;             /* Choice* */
  guint selected;
};

enum
{
  PROP_0,
  PROP_SELECTED,
  N_PROPERTIES,
};

static GParamSpec *properties[N_PROPERTIES];

G_DEFINE_FINAL_TYPE (XdOptionPicker, xd_option_picker, ADW_TYPE_BIN)

static void
choice_free (Choice *choice)
{
  g_free (choice->label);
  g_free (choice->description);
  g_free (choice);
}

static void
sync_selection (XdOptionPicker *self)
{
  Choice *selected;

  if (self->choices->len == 0 || self->selected >= self->choices->len)
    return;

  selected = g_ptr_array_index (self->choices, self->selected);
  gtk_label_set_label (self->button_label, selected->label);
  gtk_list_box_select_row (self->list, selected->row);

  for (guint i = 0; i < self->choices->len; i++)
    {
      Choice *choice = g_ptr_array_index (self->choices, i);

      gtk_widget_set_opacity (GTK_WIDGET (choice->check),
                              i == self->selected ? 1.0 : 0.0);
    }
}

guint
xd_option_picker_get_selected (XdOptionPicker *self)
{
  g_return_val_if_fail (XD_IS_OPTION_PICKER (self), 0);

  return self->selected;
}

void
xd_option_picker_set_selected (XdOptionPicker *self,
                               guint           selected)
{
  g_return_if_fail (XD_IS_OPTION_PICKER (self));
  g_return_if_fail (selected < self->choices->len);

  if (self->selected == selected)
    {
      sync_selection (self);
      return;
    }

  self->selected = selected;
  sync_selection (self);
  g_object_notify_by_pspec (G_OBJECT (self), properties[PROP_SELECTED]);
}

void
xd_option_picker_set_label (XdOptionPicker *self,
                            guint           position,
                            const char     *label)
{
  Choice *choice;

  g_return_if_fail (XD_IS_OPTION_PICKER (self));
  g_return_if_fail (position < self->choices->len);

  choice = g_ptr_array_index (self->choices, position);
  g_free (choice->label);
  choice->label = g_strdup (label);
  gtk_label_set_label (choice->row_label, choice->label);

  if (position == self->selected)
    gtk_label_set_label (self->button_label, choice->label);
}

static void
on_row_activated (GtkListBox    *list,
                  GtkListBoxRow *row,
                  gpointer       user_data)
{
  XdOptionPicker *self = user_data;
  guint position =
    GPOINTER_TO_UINT (g_object_get_data (G_OBJECT (row), "position")) - 1;

  xd_option_picker_set_selected (self, position);
  gtk_menu_button_popdown (self->button);
}

static void
append_choice (XdOptionPicker *self,
               const char     *label,
               const char     *description)
{
  Choice *choice = g_new0 (Choice, 1);
  GtkWidget *row = gtk_list_box_row_new ();
  GtkWidget *content = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 12);
  GtkWidget *text = gtk_box_new (GTK_ORIENTATION_VERTICAL, 2);
  GtkWidget *detail = gtk_label_new (description);
  guint position = self->choices->len;

  choice->label = g_strdup (label);
  choice->description = g_strdup (description);
  choice->row_label = GTK_LABEL (gtk_label_new (label));
  choice->check = GTK_IMAGE (
    gtk_image_new_from_icon_name ("object-select-symbolic"));
  choice->row = GTK_LIST_BOX_ROW (row);

  gtk_label_set_xalign (choice->row_label, 0.0f);
  gtk_label_set_xalign (GTK_LABEL (detail), 0.0f);
  gtk_label_set_wrap (GTK_LABEL (detail), TRUE);
  gtk_widget_add_css_class (detail, "caption");
  gtk_widget_add_css_class (detail, "dim-label");

  gtk_box_append (GTK_BOX (text), GTK_WIDGET (choice->row_label));
  gtk_box_append (GTK_BOX (text), detail);
  gtk_widget_set_hexpand (text, TRUE);

  gtk_widget_set_valign (GTK_WIDGET (choice->check), GTK_ALIGN_CENTER);
  gtk_box_append (GTK_BOX (content), text);
  gtk_box_append (GTK_BOX (content), GTK_WIDGET (choice->check));
  gtk_widget_set_margin_top (content, 8);
  gtk_widget_set_margin_bottom (content, 8);
  gtk_widget_set_margin_start (content, 10);
  gtk_widget_set_margin_end (content, 10);

  gtk_list_box_row_set_child (choice->row, content);
  g_object_set_data (G_OBJECT (row), "position",
                     GUINT_TO_POINTER (position + 1));
  gtk_list_box_append (self->list, row);
  g_ptr_array_add (self->choices, choice);
}

XdOptionPicker *
xd_option_picker_new (const char *const *labels,
                      const char *const *descriptions)
{
  XdOptionPicker *self;

  g_return_val_if_fail (labels != NULL, NULL);
  g_return_val_if_fail (descriptions != NULL, NULL);

  self = g_object_new (XD_TYPE_OPTION_PICKER, NULL);

  for (guint i = 0; labels[i] != NULL; i++)
    append_choice (self, labels[i], descriptions[i]);

  sync_selection (self);

  return self;
}

static void
xd_option_picker_get_property (GObject    *object,
                               guint       property_id,
                               GValue     *value,
                               GParamSpec *pspec)
{
  XdOptionPicker *self = XD_OPTION_PICKER (object);

  switch (property_id)
    {
    case PROP_SELECTED:
      g_value_set_uint (value, self->selected);
      break;

    default:
      G_OBJECT_WARN_INVALID_PROPERTY_ID (object, property_id, pspec);
    }
}

static void
xd_option_picker_set_property (GObject      *object,
                               guint         property_id,
                               const GValue *value,
                               GParamSpec   *pspec)
{
  XdOptionPicker *self = XD_OPTION_PICKER (object);

  switch (property_id)
    {
    case PROP_SELECTED:
      xd_option_picker_set_selected (self, g_value_get_uint (value));
      break;

    default:
      G_OBJECT_WARN_INVALID_PROPERTY_ID (object, property_id, pspec);
    }
}

static void
xd_option_picker_dispose (GObject *object)
{
  XdOptionPicker *self = XD_OPTION_PICKER (object);

  g_clear_pointer (&self->choices, g_ptr_array_unref);

  G_OBJECT_CLASS (xd_option_picker_parent_class)->dispose (object);
}

static void
xd_option_picker_class_init (XdOptionPickerClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->get_property = xd_option_picker_get_property;
  object_class->set_property = xd_option_picker_set_property;
  object_class->dispose = xd_option_picker_dispose;

  properties[PROP_SELECTED] =
    g_param_spec_uint ("selected", NULL, NULL, 0, G_MAXUINT, 0,
                       G_PARAM_READWRITE | G_PARAM_EXPLICIT_NOTIFY |
                       G_PARAM_STATIC_STRINGS);

  g_object_class_install_properties (object_class, N_PROPERTIES, properties);
}

static void
xd_option_picker_init (XdOptionPicker *self)
{
  GtkWidget *button_content = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
  GtkWidget *popover = gtk_popover_new ();
  GtkWidget *panel = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);

  self->choices =
    g_ptr_array_new_with_free_func ((GDestroyNotify) choice_free);
  self->button_label = GTK_LABEL (gtk_label_new (NULL));
  gtk_label_set_ellipsize (self->button_label, PANGO_ELLIPSIZE_END);

  gtk_box_append (GTK_BOX (button_content), GTK_WIDGET (self->button_label));
  gtk_box_append (GTK_BOX (button_content),
                  gtk_image_new_from_icon_name ("pan-down-symbolic"));

  self->button = GTK_MENU_BUTTON (gtk_menu_button_new ());
  gtk_menu_button_set_child (self->button, button_content);
  gtk_widget_add_css_class (GTK_WIDGET (self->button), "flat");

  self->list = GTK_LIST_BOX (gtk_list_box_new ());
  gtk_list_box_set_selection_mode (self->list, GTK_SELECTION_SINGLE);
  gtk_widget_set_size_request (GTK_WIDGET (self->list), 320, -1);
  g_signal_connect (self->list, "row-activated",
                    G_CALLBACK (on_row_activated), self);

  gtk_box_append (GTK_BOX (panel), GTK_WIDGET (self->list));
  gtk_widget_add_css_class (panel, "xd-menu");
  gtk_popover_set_child (GTK_POPOVER (popover), panel);
  gtk_popover_set_has_arrow (GTK_POPOVER (popover), FALSE);
  gtk_menu_button_set_popover (self->button, popover);

  adw_bin_set_child (ADW_BIN (self), GTK_WIDGET (self->button));
}
