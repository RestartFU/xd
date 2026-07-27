#include "message-row.h"

#include <string.h>

#include "remote/client.h"
#include "remote/protocol.h"
#include "util/markdown.h"
#include "util/host-launch.h"

struct _XdMessageRow
{
  AdwBin parent_instance;

  XdMessageKind kind;
  GString *text;
  XdRemoteClient *remote;
  GCancellable *image_cancellable;

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
#if defined(G_OS_WIN32) || defined(__APPLE__)
  g_app_info_launch_default_for_uri (uri, NULL, NULL);
#else
  g_auto (GStrv) env = xd_host_environ ();
  const char *argv[] = { "xdg-open", uri, NULL };

  g_spawn_async (NULL, (char **) argv, env, G_SPAWN_SEARCH_PATH,
                 NULL, NULL, NULL, NULL);
#endif

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
static void
fill_diff_buffer (GtkTextBuffer *buffer,
                  const char    *code)
{
  g_auto (GStrv) lines = g_strsplit (code, "\n", -1);
  GtkTextIter at;

  gtk_text_buffer_create_tag (buffer, "added", "foreground", "#57e389", NULL);
  gtk_text_buffer_create_tag (buffer, "removed", "foreground", "#f66151", NULL);
  gtk_text_buffer_create_tag (buffer, "hunk", "foreground", "#78aeed", NULL);
  gtk_text_buffer_create_tag (buffer, "header", "weight", PANGO_WEIGHT_BOLD, NULL);
  gtk_text_buffer_get_start_iter (buffer, &at);

  for (gsize i = 0; lines[i] != NULL; i++)
    {
      const char *line = lines[i];
      const char *tag = NULL;

      if (g_str_has_prefix (line, "diff ") ||
          g_str_has_prefix (line, "index ") ||
          g_str_has_prefix (line, "+++") ||
          g_str_has_prefix (line, "---") ||
          g_str_has_prefix (line, "new file") ||
          g_str_has_prefix (line, "deleted file"))
        tag = "header";
      else if (g_str_has_prefix (line, "@@"))
        tag = "hunk";
      else if (line[0] == '+')
        tag = "added";
      else if (line[0] == '-')
        tag = "removed";

      if (tag != NULL)
        gtk_text_buffer_insert_with_tags_by_name (buffer, &at, line, -1,
                                                  tag, NULL);
      else
        gtk_text_buffer_insert (buffer, &at, line, -1);

      if (lines[i + 1] != NULL)
        gtk_text_buffer_insert (buffer, &at, "\n", 1);
    }
}

static GtkWidget *
make_code_card (XdMessageRow *self,
                const char   *code,
                gboolean      diff)
{
  GtkWidget *card = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *content;

  if (diff)
    {
      GtkTextView *view = GTK_TEXT_VIEW (gtk_text_view_new ());

      gtk_text_view_set_editable (view, FALSE);
      gtk_text_view_set_cursor_visible (view, FALSE);
      gtk_text_view_set_monospace (view, TRUE);
      gtk_text_view_set_wrap_mode (view, GTK_WRAP_WORD_CHAR);
      gtk_widget_add_css_class (GTK_WIDGET (view), "xd-diff");
      fill_diff_buffer (gtk_text_view_get_buffer (view), code);
      content = GTK_WIDGET (view);
    }
  else
    {
      GtkWidget *label = gtk_label_new (code);

      gtk_label_set_wrap (GTK_LABEL (label), TRUE);
      gtk_label_set_wrap_mode (GTK_LABEL (label), PANGO_WRAP_WORD_CHAR);
      gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
      gtk_label_set_selectable (GTK_LABEL (label), TRUE);
      gtk_widget_add_css_class (label, "xd-body");
      content = label;
    }

  gtk_widget_set_hexpand (content, TRUE);

  gtk_widget_add_css_class (card, "xd-code");
  gtk_box_append (GTK_BOX (card), content);

  return card;
}

typedef struct
{
  GtkPopover *popover;
  GWeakRef chip;
} PreviewRequest;

static void
preview_request_free (PreviewRequest *request)
{
  g_weak_ref_clear (&request->chip);
  g_object_unref (request->popover);
  g_free (request);
}

/* The preview popover opens above the chip, sized to the image's own shape
 * rather than a fixed box that would stretch or letterbox it. */
static void
show_preview (GtkPopover *popover,
              GdkTexture *texture)
{
  GtkWidget *picture;
  int w = gdk_texture_get_width (texture);
  int h = gdk_texture_get_height (texture);
  double scale = MIN (1.0, MIN (440.0 / w, 300.0 / h));

  picture = gtk_picture_new_for_paintable (GDK_PAINTABLE (texture));
  gtk_picture_set_content_fit (GTK_PICTURE (picture), GTK_CONTENT_FIT_CONTAIN);
  gtk_widget_set_size_request (picture, (int) (w * scale), (int) (h * scale));
  gtk_popover_set_child (popover, picture);
}

static void
on_remote_preview (GObject      *source,
                   GAsyncResult *result,
                   gpointer      user_data)
{
  PreviewRequest *request = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree guchar *data = NULL;
  g_autoptr (GBytes) bytes = NULL;
  g_autoptr (GdkTexture) texture = NULL;
  g_autoptr (GtkWidget) chip = g_weak_ref_get (&request->chip);
  const char *encoded;
  gsize length = 0;
  gsize encoded_limit = ((XD_REMOTE_MAX_IMAGE_BYTES + 2) / 3) * 4;

  if (chip != NULL)
    g_object_set_data (G_OBJECT (chip), "image-loading", NULL);

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  encoded = reply != NULL
    ? json_object_get_string_member_with_default (reply, "data", NULL) : NULL;

  if (encoded != NULL && strlen (encoded) <= encoded_limit)
    data = g_base64_decode (encoded, &length);

  if (length > 0 && length <= XD_REMOTE_MAX_IMAGE_BYTES)
    {
      bytes = g_bytes_new_take (g_steal_pointer (&data), length);
      texture = gdk_texture_new_from_bytes (bytes, &error);
    }

  if (texture != NULL)
    {
      show_preview (request->popover, texture);
    }
  else if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    {
      GtkWidget *unavailable = gtk_label_new ("Preview unavailable");

      gtk_widget_add_css_class (unavailable, "dim-label");
      gtk_popover_set_child (request->popover, unavailable);
    }

  preview_request_free (request);
}

static void
load_remote_preview (XdMessageRow *self,
                     GtkWidget    *chip,
                     GtkPopover   *popover,
                     const char   *path)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request_node = NULL;
  PreviewRequest *request;
  GtkWidget *loading = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *spinner = gtk_spinner_new ();

  gtk_spinner_start (GTK_SPINNER (spinner));
  gtk_box_append (GTK_BOX (loading), spinner);
  gtk_box_append (GTK_BOX (loading), gtk_label_new ("Loading preview…"));
  gtk_popover_set_child (popover, loading);
  g_object_set_data (G_OBJECT (chip), "image-loading", GINT_TO_POINTER (1));

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "image-read");
  json_builder_set_member_name (builder, "path");
  json_builder_add_string_value (builder, path);
  json_builder_end_object (builder);
  request_node = json_builder_get_root (builder);

  request = g_new0 (PreviewRequest, 1);
  request->popover = g_object_ref (popover);
  g_weak_ref_init (&request->chip, chip);

  xd_remote_client_call_async (self->remote, request_node,
                               self->image_cancellable,
                               on_remote_preview, request);
}

static void
on_chip_enter (GtkEventControllerMotion *controller,
               double                    x,
               double                    y,
               gpointer                  user_data)
{
  XdMessageRow *self = user_data;
  GtkWidget *chip =
    gtk_event_controller_get_widget (GTK_EVENT_CONTROLLER (controller));
  GtkPopover *popover =
    g_object_get_data (G_OBJECT (chip), "image-popover");
  const char *path = g_object_get_data (G_OBJECT (chip), "image-path");

  if (popover == NULL || path == NULL)
    return;

  if (gtk_popover_get_child (popover) == NULL &&
      g_object_get_data (G_OBJECT (chip), "image-loading") == NULL)
    {
      g_autoptr (GdkTexture) texture = NULL;

      if (self->remote != NULL)
        load_remote_preview (self, chip, popover, path);
      else
        {
          texture = gdk_texture_new_from_filename (path, NULL);
          if (texture == NULL)
            return;
          show_preview (popover, texture);
        }
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
  GtkWidget *label = gtk_label_new (NULL);

  if (self->remote != NULL)
    {
      g_autofree char *text = g_strdup_printf ("Image #%d", number);

      gtk_label_set_label (GTK_LABEL (label), text);
      gtk_widget_add_css_class (label, "link");
    }
  else
    {
      g_autofree char *markup =
        g_markup_printf_escaped ("<a href=\"%s\">Image #%d</a>",
                                 uri != NULL ? uri : path, number);

      gtk_label_set_markup (GTK_LABEL (label), markup);
      g_signal_connect (label, "activate-link",
                        G_CALLBACK (on_link_activated), NULL);
    }
  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);

  g_object_set_data_full (G_OBJECT (label), "image-path", g_strdup (path), g_free);

  {
    GtkWidget *popover = gtk_popover_new ();
    GtkEventController *motion = gtk_event_controller_motion_new ();

    gtk_popover_set_position (GTK_POPOVER (popover), GTK_POS_TOP);
    gtk_popover_set_autohide (GTK_POPOVER (popover), FALSE);
    gtk_popover_set_has_arrow (GTK_POPOVER (popover), FALSE);
    gtk_widget_set_parent (popover, label);
    gtk_widget_add_css_class (popover, "xd-preview");
    g_object_set_data (G_OBJECT (label), "image-popover", popover);

    g_signal_connect (motion, "enter", G_CALLBACK (on_chip_enter), self);
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
    gboolean diff_fence = FALSE;

    for (gsize i = 0; lines[i] != NULL; i++)
      {
        if (g_str_has_prefix (lines[i], "```"))
          {
            if (chunk->len > 0)
              {
                GtkWidget *piece;

                if (in_fence)
                  piece = make_code_card (self, chunk->str, diff_fence);
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
            diff_fence = in_fence &&
                         g_strcmp0 (lines[i] + strlen ("```"), "diff") == 0;
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
          piece = make_code_card (self, chunk->str, diff_fence);
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

void
xd_message_row_set_remote (XdMessageRow   *self,
                           XdRemoteClient *remote)
{
  g_return_if_fail (XD_IS_MESSAGE_ROW (self));
  g_return_if_fail (remote == NULL || XD_IS_REMOTE_CLIENT (remote));

  if (self->remote == remote)
    return;

  g_cancellable_cancel (self->image_cancellable);
  g_clear_object (&self->image_cancellable);
  self->image_cancellable = g_cancellable_new ();
  g_set_object (&self->remote, remote);

  /* Image chips choose their loader when they are built. */
  render_body (self);
}

static void
xd_message_row_dispose (GObject *object)
{
  XdMessageRow *self = XD_MESSAGE_ROW (object);

  g_cancellable_cancel (self->image_cancellable);
  g_clear_object (&self->image_cancellable);
  g_clear_object (&self->remote);

  G_OBJECT_CLASS (xd_message_row_parent_class)->dispose (object);
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
  G_OBJECT_CLASS (klass)->dispose = xd_message_row_dispose;
  G_OBJECT_CLASS (klass)->finalize = xd_message_row_finalize;
}

static void
xd_message_row_init (XdMessageRow *self)
{
  self->image_cancellable = g_cancellable_new ();
}
