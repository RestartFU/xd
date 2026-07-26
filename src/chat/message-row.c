#include "message-row.h"

#include <string.h>

#include "util/markdown.h"
#include "util/host-launch.h"

struct _XdMessageRow
{
  AdwBin parent_instance;

  XdMessageKind kind;
  GString *text;

  GtkWidget *body;          /* a column of prose labels and code cards */
};

G_DEFINE_FINAL_TYPE (XdMessageRow, xd_message_row, ADW_TYPE_BIN)

static void render_body (XdMessageRow *self);

XdMessageKind
xd_message_kind_from_role (const char *role)
{
  if (g_strcmp0 (role, "assistant") == 0)
    return XD_MESSAGE_ASSISTANT;
  if (g_strcmp0 (role, "tool") == 0)
    return XD_MESSAGE_TOOL;
  /* Things that happened rather than things said: model switches and the
   * like read as dim asides, the same register as tool calls. */
  if (g_strcmp0 (role, "event") == 0)
    return XD_MESSAGE_TOOL;
  if (g_strcmp0 (role, "error") == 0)
    return XD_MESSAGE_ERROR;

  return XD_MESSAGE_USER;
}

const char *
xd_message_kind_to_role (XdMessageKind kind)
{
  switch (kind)
    {
    case XD_MESSAGE_ASSISTANT: return "assistant";
    case XD_MESSAGE_TOOL:      return "tool";
    case XD_MESSAGE_ERROR:     return "error";
    case XD_MESSAGE_USER:
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
kind_is_bubble (XdMessageKind kind)
{
  return kind == XD_MESSAGE_USER;
}

/* Roughly the width of the reply column, so a bubble wraps well before it
 * reaches the far side rather than filling the window. */
#define BUBBLE_MAX_WIDTH_CHARS 60

static const char *
kind_css_class (XdMessageKind kind)
{
  switch (kind)
    {
    case XD_MESSAGE_TOOL:  return "dim-label";
    case XD_MESSAGE_ERROR: return "error";
    default:               return NULL;
    }
}

/*
 * Opens the link with the host's browser, not the bundle's environment.
 *
 * GTK's own handler would spawn xdg-open under xd's rewritten environment,
 * and a browser launched with the bundle's GTK and schemas is a different
 * program than the one configured.
 */
static gboolean
on_link_activated (GtkLabel   *label,
                   const char *uri,
                   gpointer    user_data)
{
  g_auto (GStrv) env = xd_host_environ ();
  const char *argv[] = { "xdg-open", uri, NULL };

  g_spawn_async (NULL, (char **) argv, env, G_SPAWN_SEARCH_PATH,
                 NULL, NULL, NULL, NULL);

  return TRUE;
}

XdMessageRow *
xd_message_row_new (XdMessageKind  kind,
                    const char    *text)
{
  XdMessageRow *self = g_object_new (XD_TYPE_MESSAGE_ROW, NULL);
  GtkWidget *card = gtk_box_new (GTK_ORIENTATION_VERTICAL, 6);
  gboolean bubble = kind_is_bubble (kind);

  self->kind = kind;
  self->text = g_string_new (text != NULL ? text : "");


  self->body = gtk_box_new (GTK_ORIENTATION_VERTICAL, 8);
  render_body (self);

  gtk_box_append (GTK_BOX (card), self->body);

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
      gtk_widget_set_hexpand (self->body, FALSE);

      /* The card class carries no padding of its own. */
      gtk_widget_set_margin_top (self->body, 10);
      gtk_widget_set_margin_bottom (self->body, 10);
      gtk_widget_set_margin_start (self->body, 14);
      gtk_widget_set_margin_end (self->body, 14);
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

/* One prose label, configured the way every piece of message text is. */
static GtkWidget *
make_text_label (XdMessageRow *self)
{
  GtkWidget *label = gtk_label_new (NULL);
  const char *css_class = kind_css_class (self->kind);

  gtk_label_set_wrap (GTK_LABEL (label), TRUE);
  gtk_label_set_wrap_mode (GTK_LABEL (label), PANGO_WRAP_WORD_CHAR);
  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_label_set_selectable (GTK_LABEL (label), TRUE);
  gtk_widget_add_css_class (label, "xd-body");
  g_signal_connect (label, "activate-link", G_CALLBACK (on_link_activated), NULL);

  if (kind_is_bubble (self->kind))
    gtk_label_set_max_width_chars (GTK_LABEL (label), BUBBLE_MAX_WIDTH_CHARS);

  if (css_class != NULL)
    gtk_widget_add_css_class (label, css_class);

  return label;
}

/*
 * A fenced block gets a card of its own.
 *
 * Pango markup cannot draw a padded, rounded background behind a run of
 * text, so a code block that stays inside the label can only ever be
 * monospace prose. As a widget it can look like what it is.
 */
static GtkWidget *
make_code_card (XdMessageRow *self,
                const char   *code)
{
  GtkWidget *card = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *label = gtk_label_new (code);

  gtk_label_set_wrap (GTK_LABEL (label), TRUE);
  gtk_label_set_wrap_mode (GTK_LABEL (label), PANGO_WRAP_WORD_CHAR);
  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_label_set_selectable (GTK_LABEL (label), TRUE);
  gtk_widget_add_css_class (label, "xd-body");
  gtk_widget_set_hexpand (label, TRUE);

  gtk_widget_add_css_class (card, "xd-code");
  gtk_box_append (GTK_BOX (card), label);

  return card;
}

/* The preview popover opens above the chip, sized to the image's own shape
 * rather than a fixed box that would stretch or letterbox it. */
static void
on_chip_enter (GtkEventControllerMotion *controller,
               double                    x,
               double                    y,
               gpointer                  user_data)
{
  GtkPopover *popover = user_data;
  GtkWidget *chip = gtk_widget_get_parent (GTK_WIDGET (popover));
  const char *path = g_object_get_data (G_OBJECT (chip), "image-path");

  if (gtk_popover_get_child (popover) == NULL)
    {
      g_autoptr (GdkTexture) texture = NULL;
      GtkWidget *picture;
      int w, h;
      double scale;

      if (path == NULL)
        return;

      texture = gdk_texture_new_from_filename (path, NULL);
      if (texture == NULL)
        return;

      w = gdk_texture_get_width (GDK_TEXTURE (texture));
      h = gdk_texture_get_height (GDK_TEXTURE (texture));
      scale = MIN (1.0, MIN (440.0 / w, 300.0 / h));

      picture = gtk_picture_new_for_paintable (GDK_PAINTABLE (texture));
      gtk_picture_set_content_fit (GTK_PICTURE (picture), GTK_CONTENT_FIT_CONTAIN);
      gtk_widget_set_size_request (picture, (int) (w * scale), (int) (h * scale));
      gtk_popover_set_child (popover, picture);
    }

  gtk_popover_popup (popover);
}

static void
on_chip_leave (GtkEventControllerMotion *controller,
               gpointer                  user_data)
{
  gtk_popover_popdown (GTK_POPOVER (user_data));
}

static void
on_chip_destroyed (GtkWidget *chip,
                   gpointer   user_data)
{
  gtk_widget_unparent (GTK_WIDGET (user_data));
}

/*
 * "[image: /path]" becomes a chip that says what it is.
 *
 * The path is how the agent receives the image, but the person who pasted it
 * knows it as a picture, not a filename. The chip reads "Image #1", opens
 * with a click, and previews on hover.
 */
static GtkWidget *
make_image_chip (XdMessageRow *self,
                 const char   *path,
                 int           number)
{
  g_autofree char *uri = g_filename_to_uri (path, NULL, NULL);
  g_autofree char *markup =
    g_markup_printf_escaped ("<a href=\"%s\">Image #%d</a>",
                             uri != NULL ? uri : path, number);
  GtkWidget *label = gtk_label_new (NULL);

  gtk_label_set_markup (GTK_LABEL (label), markup);
  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  g_signal_connect (label, "activate-link", G_CALLBACK (on_link_activated), NULL);

  g_object_set_data_full (G_OBJECT (label), "image-path", g_strdup (path), g_free);

  {
    GtkWidget *popover = gtk_popover_new ();
    GtkEventController *motion = gtk_event_controller_motion_new ();

    gtk_popover_set_position (GTK_POPOVER (popover), GTK_POS_TOP);
    gtk_popover_set_autohide (GTK_POPOVER (popover), FALSE);
    gtk_popover_set_has_arrow (GTK_POPOVER (popover), FALSE);
    gtk_widget_set_parent (popover, label);
    gtk_widget_add_css_class (popover, "xd-preview");

    g_signal_connect (motion, "enter", G_CALLBACK (on_chip_enter), popover);
    g_signal_connect (motion, "leave", G_CALLBACK (on_chip_leave), popover);
    gtk_widget_add_controller (label, motion);
    g_signal_connect (label, "destroy", G_CALLBACK (on_chip_destroyed), popover);
  }

  return label;
}

static void
clear_body (XdMessageRow *self)
{
  GtkWidget *child;

  while ((child = gtk_widget_get_first_child (self->body)) != NULL)
    gtk_box_remove (GTK_BOX (self->body), child);
}

/* Replies are Markdown; other rows stay literal except that URLs are links. */
static void
render_body (XdMessageRow *self)
{
  clear_body (self);

  if (self->kind != XD_MESSAGE_ASSISTANT)
    {
      g_auto (GStrv) lines = g_strsplit (self->text->str, "\n", -1);
      g_autoptr (GString) prose = g_string_new (NULL);
      int images = 0;

      for (gsize i = 0; lines[i] != NULL; i++)
        {
          gsize len = strlen (lines[i]);

          if (g_str_has_prefix (lines[i], "[image: ") &&
              len > 9 && lines[i][len - 1] == ']')
            {
              g_autofree char *path = g_strndup (lines[i] + 8, len - 9);

              if (prose->len > 0)
                {
                  GtkWidget *label = make_text_label (self);
                  g_autofree char *markup = xd_urls_to_pango (prose->str);

                  gtk_label_set_markup (GTK_LABEL (label), markup);
                  gtk_box_append (GTK_BOX (self->body), label);
                  g_string_truncate (prose, 0);
                }

              gtk_box_append (GTK_BOX (self->body),
                              make_image_chip (self, path, ++images));
              continue;
            }

          if (prose->len > 0)
            g_string_append_c (prose, '\n');
          g_string_append (prose, lines[i]);
        }

      if (prose->len > 0)
        {
          GtkWidget *label = make_text_label (self);
          g_autofree char *markup = xd_urls_to_pango (prose->str);

          gtk_label_set_markup (GTK_LABEL (label), markup);
          gtk_box_append (GTK_BOX (self->body), label);
        }

      return;
    }

  /* Split at the fences: prose renders as markup, each fenced stretch as a
   * card. An unclosed fence -- the normal state while streaming -- is
   * treated as fenced to the end. */
  {
    g_auto (GStrv) lines = g_strsplit (self->text->str, "\n", -1);
    g_autoptr (GString) chunk = g_string_new (NULL);
    gboolean in_fence = FALSE;

    for (gsize i = 0; lines[i] != NULL; i++)
      {
        if (g_str_has_prefix (lines[i], "```"))
          {
            if (chunk->len > 0)
              {
                GtkWidget *piece;

                if (in_fence)
                  piece = make_code_card (self, chunk->str);
                else
                  {
                    g_autofree char *markup = xd_markdown_to_pango (chunk->str);

                    piece = make_text_label (self);
                    gtk_label_set_markup (GTK_LABEL (piece), markup);
                  }

                gtk_box_append (GTK_BOX (self->body), piece);
                g_string_truncate (chunk, 0);
              }

            in_fence = !in_fence;
            continue;
          }

        if (chunk->len > 0)
          g_string_append_c (chunk, '\n');
        g_string_append (chunk, lines[i]);
      }

    if (chunk->len > 0)
      {
        GtkWidget *piece;

        if (in_fence)
          piece = make_code_card (self, chunk->str);
        else
          {
            g_autofree char *markup = xd_markdown_to_pango (chunk->str);

            piece = make_text_label (self);
            gtk_label_set_markup (GTK_LABEL (piece), markup);
          }

        gtk_box_append (GTK_BOX (self->body), piece);
      }
  }
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
xd_message_row_set_source (XdMessageRow *self,
                           const char   *source)
{
  g_return_if_fail (XD_IS_MESSAGE_ROW (self));

  if (source != NULL && *source != '\0')
    gtk_widget_set_tooltip_text (GTK_WIDGET (self), source);
}

static void
xd_message_row_finalize (GObject *object)
{
  XdMessageRow *self = XD_MESSAGE_ROW (object);

  g_string_free (self->text, TRUE);

  G_OBJECT_CLASS (xd_message_row_parent_class)->finalize (object);
}

static void
xd_message_row_class_init (XdMessageRowClass *klass)
{
  G_OBJECT_CLASS (klass)->finalize = xd_message_row_finalize;
}

static void
xd_message_row_init (XdMessageRow *self)
{
}
