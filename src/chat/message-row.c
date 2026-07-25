#include "message-row.h"

#include "util/markdown.h"

struct _HyMessageRow
{
  AdwBin parent_instance;

  HyMessageKind kind;
  GString *text;

  GtkLabel *body;
  GtkWidget *spinner;
};

G_DEFINE_FINAL_TYPE (HyMessageRow, hy_message_row, ADW_TYPE_BIN)

static void render_body (HyMessageRow *self);

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

/*
 * Only what the user said is drawn as a bubble.
 *
 * Replies are long and often hold code, tables and lists, so boxing them
 * would waste the width they need and turn the transcript into a column of
 * competing cards. Sides alone say who is speaking: a bubble on the right is
 * the user, plain text on the left is everything else.
 */
static gboolean
kind_is_bubble (HyMessageKind kind)
{
  return kind == HY_MESSAGE_USER;
}

/* Roughly the width of the reply column, so a bubble wraps well before it
 * reaches the far side rather than filling the window. */
#define BUBBLE_MAX_WIDTH_CHARS 60

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
  gboolean bubble = kind_is_bubble (kind);
  const char *css_class;

  self->kind = kind;
  self->text = g_string_new (text != NULL ? text : "");

  self->spinner = gtk_spinner_new ();
  gtk_widget_set_visible (self->spinner, FALSE);
  gtk_widget_set_halign (self->spinner, GTK_ALIGN_START);

  self->body = GTK_LABEL (gtk_label_new (NULL));
  render_body (self);
  gtk_label_set_wrap (self->body, TRUE);
  gtk_label_set_wrap_mode (self->body, PANGO_WRAP_WORD_CHAR);
  gtk_label_set_xalign (self->body, 0.0f);
  gtk_label_set_selectable (self->body, TRUE);

  css_class = kind_css_class (kind);
  if (css_class != NULL)
    gtk_widget_add_css_class (GTK_WIDGET (self->body), css_class);

  gtk_box_append (GTK_BOX (card), self->spinner);
  gtk_box_append (GTK_BOX (card), GTK_WIDGET (self->body));

  gtk_widget_set_margin_top (card, 6);
  gtk_widget_set_margin_bottom (card, 6);
  gtk_widget_set_margin_start (card, 12);
  gtk_widget_set_margin_end (card, 12);

  if (bubble)
    {
      gtk_widget_add_css_class (card, "card");
      gtk_widget_set_halign (card, GTK_ALIGN_END);

      /* Shrink to the text: a short message should be a short bubble, not a
       * wide one with the words pushed left. */
      gtk_widget_set_hexpand (GTK_WIDGET (self->body), FALSE);
      gtk_label_set_max_width_chars (self->body, BUBBLE_MAX_WIDTH_CHARS);

      /* The card class carries no padding of its own. */
      gtk_widget_set_margin_top (GTK_WIDGET (self->body), 10);
      gtk_widget_set_margin_bottom (GTK_WIDGET (self->body), 10);
      gtk_widget_set_margin_start (GTK_WIDGET (self->body), 14);
      gtk_widget_set_margin_end (GTK_WIDGET (self->body), 14);
      gtk_label_set_xalign (self->body, 0.0f);
    }
  else
    {
      /* Replies keep the full width, so they read as the page rather than as
       * something pinned to one side of it. */
      gtk_widget_set_halign (card, GTK_ALIGN_FILL);
      gtk_widget_set_hexpand (card, TRUE);
      gtk_widget_set_margin_top (card, 12);
    }

  adw_bin_set_child (ADW_BIN (self), card);

  return self;
}

/* Replies are Markdown; what the user typed is shown exactly as typed. */
static void
render_body (HyMessageRow *self)
{
  if (self->kind == HY_MESSAGE_ASSISTANT)
    {
      g_autofree char *markup = hy_markdown_to_pango (self->text->str);

      gtk_label_set_markup (self->body, markup);
    }
  else
    {
      gtk_label_set_text (self->body, self->text->str);
    }
}

void
hy_message_row_append (HyMessageRow *self,
                       const char   *delta)
{
  g_return_if_fail (HY_IS_MESSAGE_ROW (self));

  if (delta == NULL || *delta == '\0')
    return;

  g_string_append (self->text, delta);
  render_body (self);

  hy_message_row_set_waiting (self, FALSE);
}

void
hy_message_row_set_text (HyMessageRow *self,
                         const char   *text)
{
  g_return_if_fail (HY_IS_MESSAGE_ROW (self));

  g_string_assign (self->text, text != NULL ? text : "");
  render_body (self);

  gtk_widget_set_visible (GTK_WIDGET (self->body), self->text->len > 0);
}

/*
 * Records what produced the message, as a tooltip.
 *
 * Which side a message is on already says whether the user or the agent
 * wrote it, so the model and effort do not need a line of their own. They
 * still answer a question that comes up after the fact -- which model said
 * this? -- so they are kept within reach rather than dropped.
 */
void
hy_message_row_set_source (HyMessageRow *self,
                           const char   *source)
{
  g_return_if_fail (HY_IS_MESSAGE_ROW (self));

  if (source != NULL && *source != '\0')
    gtk_widget_set_tooltip_text (GTK_WIDGET (self), source);
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

  /* An empty label collapses to nothing, which makes the row look broken
   * while waiting; the spinner carries the state instead. */
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
