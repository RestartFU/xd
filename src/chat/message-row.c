#include "message-row.h"

struct _HyMessageRow
{
  AdwBin parent_instance;

  HyMessageKind kind;
  GString *text;

  GtkLabel *body;
  GtkWidget *spinner;
};

G_DEFINE_FINAL_TYPE (HyMessageRow, hy_message_row, ADW_TYPE_BIN)

HyMessageKind
hy_message_kind_from_role (const char *role)
{
  if (g_strcmp0 (role, "assistant") == 0)
    return HY_MESSAGE_ASSISTANT;
  if (g_strcmp0 (role, "tool") == 0)
    return HY_MESSAGE_TOOL;
  if (g_strcmp0 (role, "error") == 0)
    return HY_MESSAGE_ERROR;

  return HY_MESSAGE_USER;
}

const char *
hy_message_kind_to_role (HyMessageKind kind)
{
  switch (kind)
    {
    case HY_MESSAGE_ASSISTANT: return "assistant";
    case HY_MESSAGE_TOOL:      return "tool";
    case HY_MESSAGE_ERROR:     return "error";
    case HY_MESSAGE_USER:
    default:                   return "user";
    }
}

static const char *
kind_title (HyMessageKind kind)
{
  switch (kind)
    {
    case HY_MESSAGE_ASSISTANT: return "Assistant";
    case HY_MESSAGE_TOOL:      return "Tool";
    case HY_MESSAGE_ERROR:     return "Error";
    case HY_MESSAGE_USER:
    default:                   return "You";
    }
}

static const char *
kind_css_class (HyMessageKind kind)
{
  switch (kind)
    {
    case HY_MESSAGE_TOOL:  return "dim-label";
    case HY_MESSAGE_ERROR: return "error";
    default:               return NULL;
    }
}

HyMessageRow *
hy_message_row_new (HyMessageKind  kind,
                    const char    *text)
{
  HyMessageRow *self = g_object_new (HY_TYPE_MESSAGE_ROW, NULL);
  GtkWidget *card = gtk_box_new (GTK_ORIENTATION_VERTICAL, 6);
  GtkWidget *header = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *title = gtk_label_new (kind_title (kind));
  const char *css_class;

  self->kind = kind;
  self->text = g_string_new (text != NULL ? text : "");

  gtk_label_set_xalign (GTK_LABEL (title), 0.0f);
  gtk_widget_add_css_class (title, "caption-heading");
  gtk_widget_add_css_class (title, "dim-label");

  self->spinner = gtk_spinner_new ();
  gtk_widget_set_visible (self->spinner, FALSE);
  gtk_widget_set_valign (self->spinner, GTK_ALIGN_CENTER);

  gtk_box_append (GTK_BOX (header), title);
  gtk_box_append (GTK_BOX (header), self->spinner);

  self->body = GTK_LABEL (gtk_label_new (self->text->str));
  gtk_label_set_wrap (self->body, TRUE);
  gtk_label_set_wrap_mode (self->body, PANGO_WRAP_WORD_CHAR);
  gtk_label_set_xalign (self->body, 0.0f);
  gtk_label_set_selectable (self->body, TRUE);

  css_class = kind_css_class (kind);
  if (css_class != NULL)
    gtk_widget_add_css_class (GTK_WIDGET (self->body), css_class);

  gtk_box_append (GTK_BOX (card), header);
  gtk_box_append (GTK_BOX (card), GTK_WIDGET (self->body));

  gtk_widget_add_css_class (card, "card");
  gtk_widget_set_margin_top (card, 6);
  gtk_widget_set_margin_bottom (card, 6);
  gtk_widget_set_margin_start (card, 12);
  gtk_widget_set_margin_end (card, 12);

  /* The card class alone has no padding; without this the text touches the
   * border. */
  gtk_widget_set_margin_top (header, 12);
  gtk_widget_set_margin_start (header, 12);
  gtk_widget_set_margin_end (header, 12);
  gtk_widget_set_margin_bottom (GTK_WIDGET (self->body), 12);
  gtk_widget_set_margin_start (GTK_WIDGET (self->body), 12);
  gtk_widget_set_margin_end (GTK_WIDGET (self->body), 12);

  adw_bin_set_child (ADW_BIN (self), card);

  return self;
}

void
hy_message_row_append (HyMessageRow *self,
                       const char   *delta)
{
  g_return_if_fail (HY_IS_MESSAGE_ROW (self));

  if (delta == NULL || *delta == '\0')
    return;

  g_string_append (self->text, delta);
  gtk_label_set_text (self->body, self->text->str);

  hy_message_row_set_waiting (self, FALSE);
}

const char *
hy_message_row_get_text (HyMessageRow *self)
{
  g_return_val_if_fail (HY_IS_MESSAGE_ROW (self), NULL);

  return self->text->str;
}

void
hy_message_row_set_waiting (HyMessageRow *self,
                            gboolean      waiting)
{
  g_return_if_fail (HY_IS_MESSAGE_ROW (self));

  gtk_widget_set_visible (self->spinner, waiting);
  gtk_spinner_set_spinning (GTK_SPINNER (self->spinner), waiting);

  /* An empty label collapses to nothing, which makes the card look broken
   * while waiting; the spinner in the header carries the state instead. */
  gtk_widget_set_visible (GTK_WIDGET (self->body), self->text->len > 0);
}

static void
hy_message_row_finalize (GObject *object)
{
  HyMessageRow *self = HY_MESSAGE_ROW (object);

  g_string_free (self->text, TRUE);

  G_OBJECT_CLASS (hy_message_row_parent_class)->finalize (object);
}

static void
hy_message_row_class_init (HyMessageRowClass *klass)
{
  G_OBJECT_CLASS (klass)->finalize = hy_message_row_finalize;
}

static void
hy_message_row_init (HyMessageRow *self)
{
}
