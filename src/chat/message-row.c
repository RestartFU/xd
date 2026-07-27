#include "message-row.h"

#include <gdk-pixbuf/gdk-pixbuf.h>
#include <string.h>

#include "remote/client.h"
#include "remote/protocol.h"
#include "diff-view.h"
#include "util/markdown.h"
#include "util/host-launch.h"
#include "util/workflow-run.h"

struct _XdMessageRow
{
  AdwBin parent_instance;

  XdMessageKind kind;
  GString *text;
  XdRemoteClient *remote;
  GCancellable *image_cancellable;
  GCancellable *workflow_cancellable;

  GtkWidget *card;
  GtkWidget *body;          /* a column of prose labels and code cards */
  GtkWidget *workflow_status;
  GtkWidget *workflow_spinner;
  GtkWidget *workflow_log;
  char *workflow_run_id;
  char *workflow_repository;
  guint workflow_poll;
};

G_DEFINE_FINAL_TYPE (XdMessageRow, xd_message_row, ADW_TYPE_BIN)

static void render_body (XdMessageRow *self);
static void clear_body (XdMessageRow *self);

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
  xd_host_open_uri (uri);

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
  self->card = card;


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

static void
make_info_card (XdMessageRow *self,
                const char   *css_class)
{
  gtk_widget_add_css_class (self->card, css_class);
  gtk_widget_set_margin_top (self->body, 12);
  gtk_widget_set_margin_bottom (self->body, 12);
  gtk_widget_set_margin_start (self->body, 14);
  gtk_widget_set_margin_end (self->body, 14);
}

static gboolean
safe_repository_component (const char *component)
{
  if (component == NULL || *component == '\0')
    return FALSE;

  for (const char *at = component; *at != '\0'; at++)
    if (!g_ascii_isalnum (*at) && *at != '-' && *at != '_' && *at != '.')
      return FALSE;

  return TRUE;
}

static char *
repository_from_workflow_url (const char *url,
                              const char *run_id)
{
  static const char prefix[] = "https://github.com/";
  g_autofree char *suffix = NULL;
  g_autofree char *repository = NULL;
  char *slash;
  gsize length;

  if (url == NULL || run_id == NULL || !g_str_has_prefix (url, prefix))
    return NULL;

  suffix = g_strdup_printf ("/actions/runs/%s", run_id);
  if (!g_str_has_suffix (url, suffix))
    return NULL;

  length = strlen (url) - strlen (prefix) - strlen (suffix);
  repository = g_strndup (url + strlen (prefix), length);
  slash = strchr (repository, '/');

  if (slash == NULL || strchr (slash + 1, '/') != NULL)
    return NULL;

  *slash = '\0';
  if (!safe_repository_component (repository) ||
      !safe_repository_component (slash + 1))
    return NULL;
  *slash = '/';

  return g_steal_pointer (&repository);
}

static const char *
workflow_conclusion_name (const char *conclusion)
{
  if (g_strcmp0 (conclusion, "success") == 0)
    return "Passed";
  if (g_strcmp0 (conclusion, "failure") == 0)
    return "Failed";
  if (g_strcmp0 (conclusion, "cancelled") == 0)
    return "Cancelled";
  if (g_strcmp0 (conclusion, "timed_out") == 0)
    return "Timed out";
  if (g_strcmp0 (conclusion, "action_required") == 0)
    return "Action required";
  if (g_strcmp0 (conclusion, "skipped") == 0)
    return "Skipped";
  if (g_strcmp0 (conclusion, "stale") == 0)
    return "Stale";

  return "Completed";
}

static void
set_workflow_status (XdMessageRow *self,
                     const char   *text,
                     gboolean      working,
                     gboolean      failed)
{
  gtk_label_set_label (GTK_LABEL (self->workflow_status), text);
  gtk_widget_set_visible (self->workflow_spinner, working);

  if (working)
    gtk_spinner_start (GTK_SPINNER (self->workflow_spinner));
  else
    gtk_spinner_stop (GTK_SPINNER (self->workflow_spinner));

  gtk_widget_remove_css_class (self->workflow_status, "success");
  gtk_widget_remove_css_class (self->workflow_status, "error");

  if (!working)
    gtk_widget_add_css_class (self->workflow_status,
                              failed ? "error" : "success");
}

typedef struct
{
  GWeakRef row;
} WorkflowStatusRequest;

static void
workflow_status_request_free (WorkflowStatusRequest *request)
{
  g_weak_ref_clear (&request->row);
  g_free (request);
}

static void refresh_workflow_status (XdMessageRow *self);

static gboolean
poll_workflow_status (gpointer user_data)
{
  XdMessageRow *self = user_data;

  refresh_workflow_status (self);
  return G_SOURCE_CONTINUE;
}

static void
on_workflow_status (GObject      *source,
                    GAsyncResult *result,
                    gpointer      user_data)
{
  WorkflowStatusRequest *request = user_data;
  g_autoptr (XdMessageRow) self = g_weak_ref_get (&request->row);
  g_autoptr (JsonParser) parser = json_parser_new ();
  g_autoptr (GError) error = NULL;
  g_autofree char *output = NULL;
  JsonNode *root_node;
  JsonObject *root;
  JsonArray *jobs;
  const char *status;
  const char *conclusion;
  guint complete = 0;
  guint total = 0;
  gboolean communicated;

  communicated = g_subprocess_communicate_utf8_finish (
    G_SUBPROCESS (source), result, &output, NULL, &error);
  workflow_status_request_free (request);
  if (self == NULL)
    return;

  g_clear_object (&self->workflow_cancellable);
  if (!communicated ||
      !g_subprocess_get_successful (G_SUBPROCESS (source)) ||
      !json_parser_load_from_data (parser, output, -1, &error))
    {
      set_workflow_status (self, "Status unavailable · retrying…", TRUE, FALSE);
      return;
    }

  root_node = json_parser_get_root (parser);
  root = root_node != NULL && JSON_NODE_HOLDS_OBJECT (root_node)
    ? json_node_get_object (root_node) : NULL;
  if (root == NULL)
    {
      set_workflow_status (self, "Status unavailable · retrying…", TRUE, FALSE);
      return;
    }

  status = json_object_get_string_member_with_default (root, "status", NULL);
  conclusion =
    json_object_get_string_member_with_default (root, "conclusion", NULL);
  jobs = json_object_has_member (root, "jobs")
    ? json_object_get_array_member (root, "jobs") : NULL;
  total = jobs != NULL ? json_array_get_length (jobs) : 0;

  for (guint i = 0; i < total; i++)
    {
      JsonObject *job = json_array_get_object_element (jobs, i);
      const char *job_status =
        json_object_get_string_member_with_default (job, "status", NULL);

      if (g_strcmp0 (job_status, "completed") == 0)
        complete++;
    }

  {
    g_autofree char *activity = xd_workflow_run_activity (jobs, 5);

    gtk_label_set_label (GTK_LABEL (self->workflow_log), activity);
    gtk_widget_set_visible (self->workflow_log, activity != NULL);
  }

  if (g_strcmp0 (status, "completed") == 0)
    {
      const char *name = workflow_conclusion_name (conclusion);
      g_autofree char *text = total > 0
        ? g_strdup_printf ("%s · %u/%u jobs completed", name, complete, total)
        : g_strdup (name);

      set_workflow_status (self, text, FALSE,
                           g_strcmp0 (conclusion, "success") != 0 &&
                           g_strcmp0 (conclusion, "skipped") != 0);

      if (self->workflow_poll != 0)
        {
          g_source_remove (self->workflow_poll);
          self->workflow_poll = 0;
        }
      return;
    }

  if (g_strcmp0 (status, "queued") == 0 ||
      g_strcmp0 (status, "requested") == 0 ||
      g_strcmp0 (status, "waiting") == 0 ||
      g_strcmp0 (status, "pending") == 0)
    {
      set_workflow_status (self, "Queued", TRUE, FALSE);
      return;
    }

  if (total > 0)
    {
      g_autofree char *text =
        g_strdup_printf ("In progress · %u/%u jobs completed", complete, total);

      set_workflow_status (self, text, TRUE, FALSE);
    }
  else
    {
      set_workflow_status (self, "In progress", TRUE, FALSE);
    }
}

static void
refresh_workflow_status (XdMessageRow *self)
{
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;
  WorkflowStatusRequest *request;

  if (self->workflow_cancellable != NULL)
    return;

  process = g_subprocess_new (
    G_SUBPROCESS_FLAGS_STDOUT_PIPE | G_SUBPROCESS_FLAGS_STDERR_SILENCE,
    &error, "gh", "run", "view", self->workflow_run_id,
    "--repo", self->workflow_repository,
    "--json", "status,conclusion,jobs", NULL);

  if (process == NULL)
    {
      set_workflow_status (self, "Status unavailable", FALSE, TRUE);
      return;
    }

  self->workflow_cancellable = g_cancellable_new ();
  request = g_new0 (WorkflowStatusRequest, 1);
  g_weak_ref_init (&request->row, self);
  g_subprocess_communicate_utf8_async (
    process, NULL, self->workflow_cancellable,
    on_workflow_status, request);
}

void
xd_message_row_make_workflow (XdMessageRow *self,
                              const char   *run_id,
                              const char   *url)
{
  GtkWidget *title;
  GtkWidget *status;
  GtkWidget *link;
  g_autofree char *title_markup = NULL;
  g_autofree char *link_markup = NULL;

  g_return_if_fail (XD_IS_MESSAGE_ROW (self));

  self->workflow_repository = repository_from_workflow_url (url, run_id);
  self->workflow_run_id = g_strdup (run_id);

  clear_body (self);
  make_info_card (self, "xd-status");

  title = gtk_label_new (NULL);
  title_markup = g_markup_printf_escaped (
    "<b>GitHub Actions · Run #%s</b>", run_id);
  gtk_label_set_markup (GTK_LABEL (title), title_markup);
  gtk_label_set_xalign (GTK_LABEL (title), 0.0f);

  status = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 7);
  self->workflow_spinner = gtk_spinner_new ();
  self->workflow_status = gtk_label_new ("Checking status…");
  self->workflow_log = gtk_label_new (NULL);
  gtk_spinner_start (GTK_SPINNER (self->workflow_spinner));
  gtk_label_set_xalign (GTK_LABEL (self->workflow_status), 0.0f);
  gtk_label_set_xalign (GTK_LABEL (self->workflow_log), 0.0f);
  gtk_label_set_selectable (GTK_LABEL (self->workflow_log), TRUE);
  gtk_label_set_ellipsize (GTK_LABEL (self->workflow_log),
                           PANGO_ELLIPSIZE_END);
  gtk_widget_set_visible (self->workflow_log, FALSE);
  gtk_widget_add_css_class (self->workflow_log, "xd-workflow-log");
  gtk_box_append (GTK_BOX (status), self->workflow_spinner);
  gtk_box_append (GTK_BOX (status), self->workflow_status);

  link = gtk_label_new (NULL);
  link_markup = g_markup_printf_escaped (
    "<a href=\"%s\">Open live status and logs</a>", url);
  gtk_label_set_markup (GTK_LABEL (link), link_markup);
  gtk_label_set_xalign (GTK_LABEL (link), 0.0f);
  g_signal_connect (link, "activate-link",
                    G_CALLBACK (on_link_activated), NULL);

  gtk_box_append (GTK_BOX (self->body), title);
  gtk_box_append (GTK_BOX (self->body), status);
  gtk_box_append (GTK_BOX (self->body), self->workflow_log);
  gtk_box_append (GTK_BOX (self->body), link);

  if (self->workflow_repository == NULL)
    {
      set_workflow_status (self, "Status unavailable", FALSE, TRUE);
      return;
    }

  refresh_workflow_status (self);
  if (self->workflow_cancellable != NULL)
    self->workflow_poll =
      g_timeout_add_seconds (10, poll_workflow_status, self);
}

void
xd_message_row_make_subagent (XdMessageRow *self,
                              GtkWidget    *activity)
{
  GtkWidget *expander;

  g_return_if_fail (XD_IS_MESSAGE_ROW (self));

  make_info_card (self, "xd-subagent");

  if (activity == NULL)
    return;

  /*
   * Claude reports a delegated agent's internal calls immediately before the
   * completed Task tool. Put that activity behind the card instead of leaving
   * a potentially huge tool list permanently in the transcript.
   */
  g_object_ref (activity);
  gtk_box_remove (GTK_BOX (gtk_widget_get_parent (activity)), activity);

  expander = gtk_expander_new (NULL);
  gtk_expander_set_expanded (GTK_EXPANDER (expander), FALSE);
  gtk_widget_set_hexpand (expander, TRUE);

  g_object_ref (self->body);
  gtk_box_remove (GTK_BOX (self->card), self->body);
  gtk_expander_set_label_widget (GTK_EXPANDER (expander), self->body);
  g_object_unref (self->body);

  gtk_expander_set_expanded (GTK_EXPANDER (activity), TRUE);
  gtk_widget_set_margin_start (activity, 12);
  gtk_widget_set_margin_end (activity, 0);
  gtk_expander_set_child (GTK_EXPANDER (expander), activity);
  g_object_unref (activity);

  gtk_box_append (GTK_BOX (self->card), expander);
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
                const char   *code,
                gboolean      diff)
{
  GtkWidget *card = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *content;

  if (diff)
    {
      GtkWidget *scroller = gtk_scrolled_window_new ();
      GtkWidget *view = xd_diff_view_new (code, TRUE, NULL, NULL);

      gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scroller),
                                      GTK_POLICY_AUTOMATIC, GTK_POLICY_NEVER);
      gtk_scrolled_window_set_propagate_natural_height (
        GTK_SCROLLED_WINDOW (scroller), TRUE);
      gtk_scrolled_window_set_child (
        GTK_SCROLLED_WINDOW (scroller), view);
      gtk_widget_add_css_class (card, "xd-inline-diff");
      content = scroller;
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
  GWeakRef stack;
  GWeakRef picture;
} PreviewRequest;

static void
preview_request_free (PreviewRequest *request)
{
  g_weak_ref_clear (&request->stack);
  g_weak_ref_clear (&request->picture);
  g_free (request);
}

#define INLINE_PREVIEW_HEIGHT 96
#define INLINE_PREVIEW_MAX_WIDTH 168

static void
prepare_preview_size (GdkPixbufLoader *loader,
                      int              width,
                      int              height,
                      gpointer         user_data)
{
  double scale = MIN (1.0, MIN ((double) INLINE_PREVIEW_MAX_WIDTH / width,
                                (double) INLINE_PREVIEW_HEIGHT / height));

  gdk_pixbuf_loader_set_size (loader,
                              MAX (1, (int) (width * scale)),
                              MAX (1, (int) (height * scale)));
}

/*
 * Ask the image decoder for thumbnail pixels. Loading a full desktop
 * screenshot into a texture and shrinking only its widget still retains the
 * large texture, which makes image-heavy transcripts expensive to reopen.
 */
static GdkTexture *
preview_texture_from_bytes (const guchar *data,
                            gsize         length,
                            GError      **error)
{
  g_autoptr (GdkPixbufLoader) loader =
    gdk_pixbuf_loader_new_with_type ("png", error);
  GdkPixbuf *pixbuf;

  if (loader == NULL)
    return NULL;

  g_signal_connect (loader, "size-prepared",
                    G_CALLBACK (prepare_preview_size), NULL);

  if (!gdk_pixbuf_loader_write (loader, data, length, error) ||
      !gdk_pixbuf_loader_close (loader, error))
    return NULL;

  pixbuf = gdk_pixbuf_loader_get_pixbuf (loader);
  if (pixbuf == NULL)
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                           "The image could not be decoded.");
      return NULL;
    }

  return gdk_texture_new_for_pixbuf (pixbuf);
}

/* Keep screenshots recognisable without letting one take over the message. */
static void
show_inline_preview (GtkPicture *picture,
                     GdkTexture *texture)
{
  int w = gdk_texture_get_width (texture);
  int h = gdk_texture_get_height (texture);
  double scale = MIN (1.0, MIN ((double) INLINE_PREVIEW_MAX_WIDTH / w,
                                (double) INLINE_PREVIEW_HEIGHT / h));

  gtk_picture_set_paintable (picture, GDK_PAINTABLE (texture));
  gtk_picture_set_content_fit (picture, GTK_CONTENT_FIT_CONTAIN);
  gtk_widget_set_size_request (GTK_WIDGET (picture),
                               MAX (1, (int) (w * scale)),
                               MAX (1, (int) (h * scale)));
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
  g_autoptr (GdkTexture) texture = NULL;
  g_autoptr (GtkWidget) stack = g_weak_ref_get (&request->stack);
  g_autoptr (GtkWidget) picture = g_weak_ref_get (&request->picture);
  const char *encoded;
  gsize length = 0;
  gsize encoded_limit = ((XD_REMOTE_MAX_IMAGE_BYTES + 2) / 3) * 4;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  encoded = reply != NULL
    ? json_object_get_string_member_with_default (reply, "data", NULL) : NULL;

  if (encoded != NULL && strlen (encoded) <= encoded_limit)
    data = g_base64_decode (encoded, &length);

  if (length > 0 && length <= XD_REMOTE_MAX_IMAGE_BYTES)
    texture = preview_texture_from_bytes (data, length, &error);

  if (texture != NULL && stack != NULL && picture != NULL)
    {
      show_inline_preview (GTK_PICTURE (picture), texture);
      gtk_stack_set_visible_child_name (GTK_STACK (stack), "picture");
    }
  else if (stack != NULL &&
           !g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    {
      gtk_stack_set_visible_child_name (GTK_STACK (stack), "unavailable");
    }

  preview_request_free (request);
}

static void
load_remote_preview (XdMessageRow *self,
                     GtkStack     *stack,
                     GtkPicture   *picture,
                     const char   *path)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request_node = NULL;
  PreviewRequest *request;

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "image-read");
  json_builder_set_member_name (builder, "path");
  json_builder_add_string_value (builder, path);
  json_builder_end_object (builder);
  request_node = json_builder_get_root (builder);

  request = g_new0 (PreviewRequest, 1);
  g_weak_ref_init (&request->stack, stack);
  g_weak_ref_init (&request->picture, picture);

  xd_remote_client_call_async (self->remote, request_node,
                               self->image_cancellable,
                               on_remote_preview, request);
}

/*
 * "[image: /path]" becomes a small inline preview.
 *
 * The path is how the agent receives the image, but the person who pasted it
 * knows it as a picture, not a filename. Keep a small preview directly beside
 * the prose so seeing it does not require a hover or open a large overlay.
 */
static GtkWidget *
make_image_preview (XdMessageRow *self,
                    const char   *path,
                    int           number)
{
  g_autofree char *uri = g_filename_to_uri (path, NULL, NULL);
  g_autoptr (GdkPixbuf) thumbnail = NULL;
  g_autoptr (GdkTexture) texture = NULL;
  GtkWidget *preview = gtk_box_new (GTK_ORIENTATION_VERTICAL, 4);
  GtkWidget *stack = gtk_stack_new ();
  GtkWidget *picture = gtk_picture_new ();
  GtkWidget *loading = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
  GtkWidget *spinner = gtk_spinner_new ();
  GtkWidget *unavailable = gtk_label_new ("Preview unavailable");
  GtkWidget *label = gtk_label_new (NULL);

  gtk_spinner_start (GTK_SPINNER (spinner));
  gtk_box_append (GTK_BOX (loading), spinner);
  gtk_box_append (GTK_BOX (loading), gtk_label_new ("Loading image…"));
  gtk_widget_add_css_class (loading, "dim-label");
  gtk_widget_add_css_class (unavailable, "dim-label");

  gtk_stack_add_named (GTK_STACK (stack), loading, "loading");
  gtk_stack_add_named (GTK_STACK (stack), picture, "picture");
  gtk_stack_add_named (GTK_STACK (stack), unavailable, "unavailable");
  gtk_stack_set_visible_child_name (GTK_STACK (stack), "loading");
  gtk_widget_add_css_class (stack, "xd-inline-image");
  gtk_widget_set_halign (stack, GTK_ALIGN_START);

  if (self->remote != NULL)
    {
      g_autofree char *text = g_strdup_printf ("Image #%d", number);

      gtk_label_set_label (GTK_LABEL (label), text);
      gtk_widget_add_css_class (label, "dim-label");
      load_remote_preview (self, GTK_STACK (stack), GTK_PICTURE (picture), path);
    }
  else
    {
      g_autofree char *markup =
        g_markup_printf_escaped ("<a href=\"%s\">Image #%d</a>",
                                 uri != NULL ? uri : path, number);

      gtk_label_set_markup (GTK_LABEL (label), markup);
      g_signal_connect (label, "activate-link",
                        G_CALLBACK (on_link_activated), NULL);

      thumbnail = gdk_pixbuf_new_from_file_at_scale (
        path, INLINE_PREVIEW_MAX_WIDTH, INLINE_PREVIEW_HEIGHT, TRUE, NULL);
      if (thumbnail != NULL)
        texture = gdk_texture_new_for_pixbuf (thumbnail);
      if (texture != NULL)
        {
          show_inline_preview (GTK_PICTURE (picture), texture);
          gtk_stack_set_visible_child_name (GTK_STACK (stack), "picture");
        }
      else
        {
          gtk_stack_set_visible_child_name (GTK_STACK (stack), "unavailable");
        }
    }
  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_widget_add_css_class (label, "caption");
  gtk_box_append (GTK_BOX (preview), stack);
  gtk_box_append (GTK_BOX (preview), label);
  gtk_widget_set_halign (preview, GTK_ALIGN_START);

  return preview;
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
                              make_image_preview (self, path, ++images));
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

  if (self->workflow_poll != 0)
    {
      g_source_remove (self->workflow_poll);
      self->workflow_poll = 0;
    }

  g_cancellable_cancel (self->image_cancellable);
  if (self->workflow_cancellable != NULL)
    g_cancellable_cancel (self->workflow_cancellable);
  g_clear_object (&self->image_cancellable);
  g_clear_object (&self->workflow_cancellable);
  g_clear_object (&self->remote);

  G_OBJECT_CLASS (xd_message_row_parent_class)->dispose (object);
}

static void
xd_message_row_finalize (GObject *object)
{
  XdMessageRow *self = XD_MESSAGE_ROW (object);

  g_string_free (self->text, TRUE);
  g_free (self->workflow_run_id);
  g_free (self->workflow_repository);

  G_OBJECT_CLASS (xd_message_row_parent_class)->finalize (object);
}

static void
xd_message_row_map (GtkWidget *widget)
{
  XdMessageRow *self = XD_MESSAGE_ROW (widget);

  GTK_WIDGET_CLASS (xd_message_row_parent_class)->map (widget);

  /* Cached transcript pages are unmapped while another chat is visible.
   * Resume their workflow status only when the page comes back on screen. */
  if (self->workflow_run_id != NULL &&
      self->workflow_repository != NULL &&
      self->workflow_poll == 0)
    {
      refresh_workflow_status (self);
      self->workflow_poll =
        g_timeout_add_seconds (10, poll_workflow_status, self);
    }
}

static void
xd_message_row_unmap (GtkWidget *widget)
{
  XdMessageRow *self = XD_MESSAGE_ROW (widget);

  g_clear_handle_id (&self->workflow_poll, g_source_remove);
  GTK_WIDGET_CLASS (xd_message_row_parent_class)->unmap (widget);
}

static void
xd_message_row_class_init (XdMessageRowClass *klass)
{
  GtkWidgetClass *widget_class = GTK_WIDGET_CLASS (klass);

  G_OBJECT_CLASS (klass)->dispose = xd_message_row_dispose;
  G_OBJECT_CLASS (klass)->finalize = xd_message_row_finalize;
  widget_class->map = xd_message_row_map;
  widget_class->unmap = xd_message_row_unmap;
}

static void
xd_message_row_init (XdMessageRow *self)
{
  self->image_cancellable = g_cancellable_new ();
}
