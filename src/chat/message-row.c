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

static XdMessageRow *
message_row_new (XdMessageKind   kind,
                 const char     *text,
                 XdRemoteClient *remote)
{
  XdMessageRow *self = g_object_new (XD_TYPE_MESSAGE_ROW, NULL);
  GtkWidget *card = gtk_box_new (GTK_ORIENTATION_VERTICAL, 6);
  gboolean bubble = kind_is_bubble (kind);

  self->kind = kind;
  self->text = g_string_new (text != NULL ? text : "");
  self->card = card;
  self->remote = remote != NULL ? g_object_ref (remote) : NULL;

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

XdMessageRow *
xd_message_row_new (XdMessageKind  kind,
                    const char    *text)
{
  return message_row_new (kind, text, NULL);
}

/*
 * Remote must be known before the first render. Constructing a local row and
 * changing it afterwards starts one local image-loader thread per attachment,
 * only to cancel every one and download the same images from the daemon.
 */
XdMessageRow *
xd_message_row_new_remote (XdMessageKind   kind,
                           const char     *text,
                           XdRemoteClient *remote)
{
  g_return_val_if_fail (XD_IS_REMOTE_CLIENT (remote), NULL);

  return message_row_new (kind, text, remote);
}

/*
 * Redraws one assistant row after streaming has been quiet for a moment.
 *
 * The caller deliberately throttles this: rebuilding Markdown for every token
 * is expensive and makes partially written syntax flash between forms.
 */
void
xd_message_row_set_text (XdMessageRow *self,
                         const char   *text)
{
  g_return_if_fail (XD_IS_MESSAGE_ROW (self));

  if (g_strcmp0 (self->text->str, text) == 0)
    return;

  g_string_assign (self->text, text != NULL ? text : "");
  render_body (self);
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
  g_autofree char *title_markup = NULL;

  g_return_if_fail (XD_IS_MESSAGE_ROW (self));

  self->workflow_repository = repository_from_workflow_url (url, run_id);
  self->workflow_run_id = g_strdup (run_id);

  clear_body (self);
  make_info_card (self, "xd-status");

  title = gtk_label_new (NULL);
  title_markup = g_markup_printf_escaped (
    "<b>GitHub Actions · Run <a href=\"%s\">#%s</a></b>", url, run_id);
  gtk_label_set_markup (GTK_LABEL (title), title_markup);
  gtk_label_set_xalign (GTK_LABEL (title), 0.0f);
  g_signal_connect (title, "activate-link",
                    G_CALLBACK (on_link_activated), NULL);

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

  gtk_box_append (GTK_BOX (self->body), title);
  gtk_box_append (GTK_BOX (self->body), status);
  gtk_box_append (GTK_BOX (self->body), self->workflow_log);

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

static void
on_subagent_toggled (GtkToggleButton *toggle,
                     gpointer         user_data)
{
  GtkImage *indicator = GTK_IMAGE (user_data);

  gtk_image_set_from_icon_name (
    indicator,
    gtk_toggle_button_get_active (toggle)
      ? "pan-down-symbolic"
      : "pan-end-symbolic");
  gtk_widget_set_tooltip_text (
    GTK_WIDGET (toggle),
    gtk_toggle_button_get_active (toggle)
      ? "Hide subagent activity"
      : "Show subagent activity");
}

void
xd_message_row_make_subagent (XdMessageRow *self,
                              GtkWidget    *activity)
{
  GtkWidget *toggle;
  GtkWidget *header;
  GtkWidget *indicator;

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

  toggle = gtk_toggle_button_new ();
  header = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  indicator = gtk_image_new_from_icon_name ("pan-end-symbolic");
  gtk_widget_add_css_class (toggle, "xd-subagent-toggle");
  gtk_widget_set_tooltip_text (toggle, "Show subagent activity");
  gtk_widget_set_hexpand (toggle, TRUE);
  gtk_widget_set_hexpand (header, TRUE);
  gtk_widget_set_valign (indicator, GTK_ALIGN_START);
  gtk_widget_set_margin_top (indicator, 3);
  gtk_widget_set_margin_top (header, 12);
  gtk_widget_set_margin_bottom (header, 12);
  gtk_widget_set_margin_start (header, 14);
  gtk_widget_set_margin_end (header, 14);

  g_object_ref (self->body);
  gtk_box_remove (GTK_BOX (self->card), self->body);
  gtk_widget_set_margin_top (self->body, 0);
  gtk_widget_set_margin_bottom (self->body, 0);
  gtk_widget_set_margin_start (self->body, 0);
  gtk_widget_set_margin_end (self->body, 0);
  gtk_box_append (GTK_BOX (header), indicator);
  gtk_box_append (GTK_BOX (header), self->body);
  g_object_unref (self->body);
  gtk_button_set_child (GTK_BUTTON (toggle), header);

  gtk_widget_set_margin_start (activity, 12);
  gtk_widget_set_margin_end (activity, 0);
  g_object_bind_property (toggle, "active",
                          activity, "expanded",
                          G_BINDING_SYNC_CREATE);
  g_object_bind_property (toggle, "active",
                          activity, "visible",
                          G_BINDING_SYNC_CREATE);
  g_signal_connect (toggle, "toggled",
                    G_CALLBACK (on_subagent_toggled), indicator);

  gtk_box_append (GTK_BOX (self->card), toggle);
  gtk_box_append (GTK_BOX (self->card), activity);
  g_object_unref (activity);
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
 * A fenced block or table grid gets a card of its own.
 *
 * Pango markup cannot draw a padded, rounded background behind a run of
 * text, so a code block that stays inside the label can only ever be
 * monospace prose. As a widget it can look like what it is.
 */
static GtkWidget *
make_code_card (XdMessageRow *self,
                const char   *code,
                gboolean      diff,
                gboolean      wrap)
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

      if (wrap)
        {
          content = label;
        }
      else
        {
          GtkWidget *scroller = gtk_scrolled_window_new ();

          gtk_label_set_wrap (GTK_LABEL (label), FALSE);
          gtk_scrolled_window_set_policy (
            GTK_SCROLLED_WINDOW (scroller),
            GTK_POLICY_AUTOMATIC, GTK_POLICY_NEVER);
          gtk_scrolled_window_set_propagate_natural_height (
            GTK_SCROLLED_WINDOW (scroller), TRUE);
          gtk_scrolled_window_set_child (
            GTK_SCROLLED_WINDOW (scroller), label);
          content = scroller;
        }
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

typedef struct
{
  char *path;
  XdRemoteClient *remote;
} ImageOpenRequest;

typedef struct
{
  GWeakRef stack;
  GWeakRef picture;
  GCancellable *cancellable;
} ImageLoadRequest;

typedef struct
{
  char *path;
  GBytes *bytes;
} ImageDecodeRequest;

static void
preview_request_free (PreviewRequest *request)
{
  g_weak_ref_clear (&request->stack);
  g_weak_ref_clear (&request->picture);
  g_free (request);
}

static void
image_open_request_free (ImageOpenRequest *request)
{
  g_clear_pointer (&request->path, g_free);
  g_clear_object (&request->remote);
  g_free (request);
}

static void
image_load_request_free (ImageLoadRequest *request)
{
  g_weak_ref_clear (&request->stack);
  g_weak_ref_clear (&request->picture);
  g_clear_object (&request->cancellable);
  g_free (request);
}

static void
image_decode_request_free (ImageDecodeRequest *request)
{
  g_clear_pointer (&request->path, g_free);
  g_clear_pointer (&request->bytes, g_bytes_unref);
  g_free (request);
}

#define INLINE_PREVIEW_HEIGHT 96
#define INLINE_PREVIEW_MAX_WIDTH 168
#define IMAGE_VIEWER_MAX_WIDTH 1920
#define IMAGE_VIEWER_MAX_HEIGHT 1200

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

static void
prepare_viewer_size (GdkPixbufLoader *loader,
                     int              width,
                     int              height,
                     gpointer         user_data)
{
  double scale = MIN (1.0, MIN ((double) IMAGE_VIEWER_MAX_WIDTH / width,
                                (double) IMAGE_VIEWER_MAX_HEIGHT / height));

  gdk_pixbuf_loader_set_size (loader,
                              MAX (1, (int) (width * scale)),
                              MAX (1, (int) (height * scale)));
}

/* Keep screenshots recognisable without letting one take over the message. */
static void
show_inline_preview (GtkStack   *stack,
                     GtkPicture *picture,
                     GdkTexture *texture)
{
  int w = gdk_texture_get_width (texture);
  int h = gdk_texture_get_height (texture);
  double scale = MIN (1.0, MIN ((double) INLINE_PREVIEW_MAX_WIDTH / w,
                                (double) INLINE_PREVIEW_HEIGHT / h));
  int preview_width = MAX (1, (int) (w * scale));
  int preview_height = MAX (1, (int) (h * scale));

  gtk_picture_set_paintable (picture, GDK_PAINTABLE (texture));

  /*
   * Scaled down, never up.
   *
   * A picture that may scale up answers "how tall are you?" with the height
   * its aspect ratio needs at the *whole* width it is offered -- the width of
   * the transcript column, not the width of the thumbnail. The bubble was then
   * given hundreds of pixels of height for a 96-pixel image and painted the
   * remainder as empty card. The texture is already thumbnail-sized, so there
   * is nothing to gain by growing it.
   */
  gtk_picture_set_content_fit (picture, GTK_CONTENT_FIT_SCALE_DOWN);
  gtk_widget_set_size_request (
    GTK_WIDGET (picture), preview_width, preview_height);

  /*
   * The stack sits in a wrapping message bubble. Pin its visible page to the
   * thumbnail too: asking only the picture left the stack free to retain a
   * much taller allocation from its other pages and stretched the card below
   * wide, shallow screenshots.
   */
  gtk_widget_set_size_request (
    GTK_WIDGET (stack), preview_width, preview_height);
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
      show_inline_preview (
        GTK_STACK (stack), GTK_PICTURE (picture), texture);
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
  json_builder_set_member_name (builder, "preview");
  json_builder_add_boolean_value (builder, TRUE);
  json_builder_end_object (builder);
  request_node = json_builder_get_root (builder);

  request = g_new0 (PreviewRequest, 1);
  g_weak_ref_init (&request->stack, stack);
  g_weak_ref_init (&request->picture, picture);

  xd_remote_client_call_async (self->remote, request_node,
                               self->image_cancellable,
                               on_remote_preview, request);
}

static void
load_local_preview_thread (GTask        *task,
                           gpointer      source_object,
                           gpointer      task_data,
                           GCancellable *cancellable)
{
  const char *path = task_data;
  g_autoptr (GError) error = NULL;
  GdkPixbuf *thumbnail;

  if (g_task_return_error_if_cancelled (task))
    return;

  thumbnail = gdk_pixbuf_new_from_file_at_scale (
    path, INLINE_PREVIEW_MAX_WIDTH, INLINE_PREVIEW_HEIGHT, TRUE, &error);

  if (thumbnail == NULL)
    {
      g_task_return_error (task, g_steal_pointer (&error));
      return;
    }

  if (g_task_return_error_if_cancelled (task))
    {
      g_object_unref (thumbnail);
      return;
    }

  g_task_return_pointer (task, thumbnail, g_object_unref);
}

static void
on_local_preview (GObject      *source,
                  GAsyncResult *result,
                  gpointer      user_data)
{
  PreviewRequest *request = user_data;
  g_autoptr (GError) error = NULL;
  g_autoptr (GdkPixbuf) thumbnail =
    g_task_propagate_pointer (G_TASK (result), &error);
  g_autoptr (GdkTexture) texture = thumbnail != NULL
    ? gdk_texture_new_for_pixbuf (thumbnail) : NULL;
  g_autoptr (GtkWidget) stack = g_weak_ref_get (&request->stack);
  g_autoptr (GtkWidget) picture = g_weak_ref_get (&request->picture);

  if (texture != NULL && stack != NULL && picture != NULL)
    {
      show_inline_preview (
        GTK_STACK (stack), GTK_PICTURE (picture), texture);
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
load_local_preview (XdMessageRow *self,
                    GtkStack     *stack,
                    GtkPicture   *picture,
                    const char   *path)
{
  g_autoptr (GTask) task = NULL;
  PreviewRequest *request = g_new0 (PreviewRequest, 1);

  g_weak_ref_init (&request->stack, stack);
  g_weak_ref_init (&request->picture, picture);

  task = g_task_new (NULL, self->image_cancellable,
                     on_local_preview, request);
  g_task_set_task_data (task, g_strdup (path), g_free);
  g_task_run_in_thread (task, load_local_preview_thread);
}

static void
load_viewer_image_thread (GTask        *task,
                          gpointer      source_object,
                          gpointer      task_data,
                          GCancellable *cancellable)
{
  ImageDecodeRequest *decode = task_data;
  g_autoptr (GdkPixbuf) pixbuf = NULL;
  g_autoptr (GError) error = NULL;

  if (g_task_return_error_if_cancelled (task))
    return;

  if (decode->bytes != NULL)
    {
      g_autoptr (GdkPixbufLoader) loader = gdk_pixbuf_loader_new ();
      gconstpointer data;
      gsize length;

      g_signal_connect (loader, "size-prepared",
                        G_CALLBACK (prepare_viewer_size), NULL);
      data = g_bytes_get_data (decode->bytes, &length);

      if (!gdk_pixbuf_loader_write (loader, data, length, &error) ||
          !gdk_pixbuf_loader_close (loader, &error))
        {
          g_task_return_error (task, g_steal_pointer (&error));
          return;
        }

      if (gdk_pixbuf_loader_get_pixbuf (loader) != NULL)
        pixbuf = g_object_ref (gdk_pixbuf_loader_get_pixbuf (loader));
    }
  else
    {
      pixbuf = gdk_pixbuf_new_from_file_at_scale (
        decode->path, IMAGE_VIEWER_MAX_WIDTH, IMAGE_VIEWER_MAX_HEIGHT,
        TRUE, &error);
    }

  if (pixbuf == NULL)
    {
      if (error == NULL)
        g_set_error_literal (&error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                             "The image could not be decoded.");
      g_task_return_error (task, g_steal_pointer (&error));
      return;
    }

  if (g_task_return_error_if_cancelled (task))
    return;

  g_task_return_pointer (task, g_steal_pointer (&pixbuf), g_object_unref);
}

static void
on_viewer_image_loaded (GObject      *source,
                        GAsyncResult *result,
                        gpointer      user_data)
{
  ImageLoadRequest *request = user_data;
  g_autoptr (GError) error = NULL;
  g_autoptr (GdkPixbuf) pixbuf =
    g_task_propagate_pointer (G_TASK (result), &error);
  g_autoptr (GdkTexture) texture = pixbuf != NULL
    ? gdk_texture_new_for_pixbuf (pixbuf) : NULL;
  g_autoptr (GtkWidget) stack = g_weak_ref_get (&request->stack);
  g_autoptr (GtkWidget) picture = g_weak_ref_get (&request->picture);

  if (texture != NULL && stack != NULL && picture != NULL)
    {
      gtk_picture_set_paintable (GTK_PICTURE (picture),
                                 GDK_PAINTABLE (texture));
      gtk_stack_set_visible_child_name (GTK_STACK (stack), "picture");
    }
  else if (stack != NULL &&
           !g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    {
      gtk_stack_set_visible_child_name (GTK_STACK (stack), "unavailable");
    }

  image_load_request_free (request);
}

static void
start_viewer_decode (ImageLoadRequest   *request,
                     ImageDecodeRequest *decode)
{
  g_autoptr (GTask) task = NULL;

  task = g_task_new (NULL, request->cancellable,
                     on_viewer_image_loaded, request);
  g_task_set_task_data (
    task, decode, (GDestroyNotify) image_decode_request_free);
  g_task_run_in_thread (task, load_viewer_image_thread);
}

static void
on_remote_viewer_image (GObject      *source,
                        GAsyncResult *result,
                        gpointer      user_data)
{
  ImageLoadRequest *request = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree guchar *data = NULL;
  g_autoptr (GtkWidget) stack = NULL;
  ImageDecodeRequest *decode;
  const char *encoded;
  gsize length = 0;
  gsize encoded_limit = ((XD_REMOTE_MAX_IMAGE_BYTES + 2) / 3) * 4;

  reply = xd_remote_client_call_finish (
    XD_REMOTE_CLIENT (source), result, &error);
  encoded = reply != NULL
    ? json_object_get_string_member_with_default (reply, "data", NULL) : NULL;

  if (encoded != NULL && strlen (encoded) <= encoded_limit)
    data = g_base64_decode (encoded, &length);

  if (length == 0 || length > XD_REMOTE_MAX_IMAGE_BYTES)
    {
      stack = g_weak_ref_get (&request->stack);
      if (stack != NULL &&
          !g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        gtk_stack_set_visible_child_name (
          GTK_STACK (stack), "unavailable");
      image_load_request_free (request);
      return;
    }

  decode = g_new0 (ImageDecodeRequest, 1);
  decode->bytes = g_bytes_new_take (g_steal_pointer (&data), length);
  start_viewer_decode (request, decode);
}

static ImageLoadRequest *
image_load_request_new (GtkStack     *stack,
                        GtkPicture   *picture,
                        GCancellable *cancellable)
{
  ImageLoadRequest *request = g_new0 (ImageLoadRequest, 1);

  g_weak_ref_init (&request->stack, stack);
  g_weak_ref_init (&request->picture, picture);
  request->cancellable = g_object_ref (cancellable);

  return request;
}

static void
close_image_viewer (GtkButton *button,
                    gpointer   user_data)
{
  adw_dialog_close (ADW_DIALOG (user_data));
}

static void
on_image_viewer_background_pressed (GtkGestureClick *gesture,
                                    int              n_press,
                                    double           x,
                                    double           y,
                                    gpointer         user_data)
{
  GtkWidget *background =
    gtk_event_controller_get_widget (GTK_EVENT_CONTROLLER (gesture));
  GtkWidget *picture =
    g_object_get_data (G_OBJECT (background), "image-picture");
  GtkWidget *target =
    gtk_widget_pick (background, x, y, GTK_PICK_DEFAULT);

  /* Anywhere but the picture is the dimmed window: clicking it closes, the
   * same as clicking past the edge of the viewer. */
  if (target != picture)
    adw_dialog_close (ADW_DIALOG (user_data));
}

static void
open_image_viewer (GtkButton *button,
                   gpointer   user_data)
{
  ImageOpenRequest *open_request = user_data;
  g_autoptr (GCancellable) cancellable = g_cancellable_new ();
  AdwDialog *dialog = ADW_DIALOG (adw_dialog_new ());
  GtkWidget *overlay = gtk_overlay_new ();
  GtkWidget *stack = gtk_stack_new ();
  GtkWidget *picture = gtk_picture_new ();
  GtkWidget *spinner = gtk_spinner_new ();
  GtkWidget *unavailable = gtk_label_new ("Image unavailable");
  GtkWidget *close = gtk_button_new_from_icon_name ("window-close-symbolic");
  GtkGesture *background_click = gtk_gesture_click_new ();
  ImageLoadRequest *load_request;

  gtk_picture_set_content_fit (GTK_PICTURE (picture), GTK_CONTENT_FIT_CONTAIN);
  gtk_widget_set_halign (picture, GTK_ALIGN_CENTER);
  gtk_widget_set_valign (picture, GTK_ALIGN_CENTER);
  gtk_widget_set_margin_top (picture, 24);
  gtk_widget_set_margin_bottom (picture, 24);
  gtk_widget_set_margin_start (picture, 24);
  gtk_widget_set_margin_end (picture, 24);

  gtk_spinner_start (GTK_SPINNER (spinner));
  gtk_widget_set_halign (spinner, GTK_ALIGN_CENTER);
  gtk_widget_set_valign (spinner, GTK_ALIGN_CENTER);
  gtk_widget_add_css_class (unavailable, "dim-label");

  gtk_stack_add_named (GTK_STACK (stack), spinner, "loading");
  gtk_stack_add_named (GTK_STACK (stack), picture, "picture");
  gtk_stack_add_named (GTK_STACK (stack), unavailable, "unavailable");
  gtk_stack_set_transition_type (
    GTK_STACK (stack), GTK_STACK_TRANSITION_TYPE_NONE);
  gtk_stack_set_visible_child_name (GTK_STACK (stack), "loading");
  gtk_widget_set_hexpand (stack, TRUE);
  gtk_widget_set_vexpand (stack, TRUE);
  gtk_widget_add_css_class (stack, "xd-image-viewer");
  g_object_set_data (G_OBJECT (stack), "image-picture", picture);
  g_signal_connect (
    background_click, "pressed",
    G_CALLBACK (on_image_viewer_background_pressed), dialog);
  gtk_widget_add_controller (
    stack, GTK_EVENT_CONTROLLER (background_click));

  gtk_widget_add_css_class (close, "circular");
  gtk_widget_add_css_class (close, "osd");
  gtk_widget_set_halign (close, GTK_ALIGN_END);
  gtk_widget_set_valign (close, GTK_ALIGN_START);
  gtk_widget_set_margin_top (close, 12);
  gtk_widget_set_margin_end (close, 12);
  gtk_widget_set_tooltip_text (close, "Close");
  g_signal_connect (close, "clicked",
                    G_CALLBACK (close_image_viewer), dialog);

  gtk_overlay_set_child (GTK_OVERLAY (overlay), stack);
  gtk_overlay_add_overlay (GTK_OVERLAY (overlay), close);

  /*
   * A dialog, so the window behind it is dimmed rather than left at full
   * brightness beside the picture. Its own sheet paints nothing (see
   * .xd-image-dialog): what is behind stays readable, just darkened, and
   * clicking it puts the picture away.
   */
  adw_dialog_set_title (dialog, "Image");
  adw_dialog_set_content_width (dialog, 1100);
  adw_dialog_set_content_height (dialog, 720);
  adw_dialog_set_child (dialog, overlay);
  gtk_widget_add_css_class (GTK_WIDGET (dialog), "xd-image-dialog");

  g_signal_connect_swapped (dialog, "closed",
                            G_CALLBACK (g_cancellable_cancel), cancellable);
  g_object_set_data_full (G_OBJECT (dialog), "image-cancellable",
                          g_object_ref (cancellable), g_object_unref);

  load_request = image_load_request_new (
    GTK_STACK (stack), GTK_PICTURE (picture), cancellable);

  if (open_request->remote != NULL)
    {
      g_autoptr (JsonBuilder) builder = json_builder_new ();
      g_autoptr (JsonNode) request_node = NULL;

      json_builder_begin_object (builder);
      json_builder_set_member_name (builder, "op");
      json_builder_add_string_value (builder, "image-read");
      json_builder_set_member_name (builder, "path");
      json_builder_add_string_value (builder, open_request->path);
      json_builder_end_object (builder);
      request_node = json_builder_get_root (builder);

      xd_remote_client_call_async (
        open_request->remote, request_node, cancellable,
        on_remote_viewer_image, load_request);
    }
  else
    {
      ImageDecodeRequest *decode = g_new0 (ImageDecodeRequest, 1);

      decode->path = g_strdup (open_request->path);
      start_viewer_decode (load_request, decode);
    }

  adw_dialog_present (dialog, GTK_WIDGET (button));
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
  GtkWidget *preview = gtk_box_new (GTK_ORIENTATION_VERTICAL, 4);
  GtkWidget *stack = gtk_stack_new ();
  GtkWidget *picture = gtk_picture_new ();
  GtkWidget *button = gtk_button_new ();
  GtkWidget *loading = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
  GtkWidget *spinner = gtk_spinner_new ();
  GtkWidget *unavailable = gtk_label_new ("Preview unavailable");
  GtkWidget *label = gtk_label_new (NULL);
  ImageOpenRequest *open_request = g_new0 (ImageOpenRequest, 1);

  gtk_spinner_start (GTK_SPINNER (spinner));
  gtk_box_append (GTK_BOX (loading), spinner);
  gtk_box_append (GTK_BOX (loading), gtk_label_new ("Loading image…"));
  gtk_widget_add_css_class (loading, "dim-label");
  gtk_widget_add_css_class (unavailable, "dim-label");

  gtk_stack_add_named (GTK_STACK (stack), loading, "loading");
  gtk_stack_add_named (GTK_STACK (stack), picture, "picture");
  gtk_stack_add_named (GTK_STACK (stack), unavailable, "unavailable");
  gtk_stack_set_hhomogeneous (GTK_STACK (stack), FALSE);
  gtk_stack_set_vhomogeneous (GTK_STACK (stack), FALSE);
  gtk_stack_set_transition_type (
    GTK_STACK (stack), GTK_STACK_TRANSITION_TYPE_NONE);
  gtk_stack_set_visible_child_name (GTK_STACK (stack), "loading");
  gtk_widget_add_css_class (stack, "xd-inline-image");
  gtk_widget_set_halign (stack, GTK_ALIGN_START);
  gtk_widget_set_valign (stack, GTK_ALIGN_START);
  gtk_widget_set_vexpand (stack, FALSE);

  open_request->path = g_strdup (path);
  open_request->remote =
    self->remote != NULL ? g_object_ref (self->remote) : NULL;
  gtk_button_set_child (GTK_BUTTON (button), stack);
  gtk_widget_add_css_class (button, "flat");
  gtk_widget_add_css_class (button, "xd-image-button");
  gtk_widget_set_halign (button, GTK_ALIGN_START);
  gtk_widget_set_valign (button, GTK_ALIGN_START);
  gtk_widget_set_tooltip_text (button, "Open image");
  g_object_set_data_full (
    G_OBJECT (button), "image-open-request", open_request,
    (GDestroyNotify) image_open_request_free);
  g_signal_connect (button, "clicked",
                    G_CALLBACK (open_image_viewer), open_request);

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
      load_local_preview (
        self, GTK_STACK (stack), GTK_PICTURE (picture), path);
    }
  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_widget_add_css_class (label, "caption");
  gtk_box_append (GTK_BOX (preview), button);
  gtk_box_append (GTK_BOX (preview), label);
  gtk_widget_set_halign (preview, GTK_ALIGN_START);
  gtk_widget_set_valign (preview, GTK_ALIGN_START);
  gtk_widget_set_vexpand (preview, FALSE);

  return preview;
}

static void
clear_body (XdMessageRow *self)
{
  GtkWidget *child;

  while ((child = gtk_widget_get_first_child (self->body)) != NULL)
    gtk_box_remove (GTK_BOX (self->body), child);
}

static gboolean
line_is_blank (const char *line)
{
  for (const char *at = line; *at != '\0'; at++)
    if (!g_ascii_isspace (*at))
      return FALSE;

  return TRUE;
}

static void
append_line (GString    *text,
             const char *line)
{
  if (text->len > 0)
    g_string_append_c (text, '\n');
  g_string_append (text, line);
}

static void
append_markdown_prose (XdMessageRow *self,
                       GString      *prose)
{
  GtkWidget *label;
  g_autofree char *markup = NULL;

  while (prose->len > 0 && prose->str[prose->len - 1] == '\n')
    g_string_truncate (prose, prose->len - 1);
  if (prose->len == 0)
    return;

  markup = xd_markdown_to_pango (prose->str);
  label = make_text_label (self);
  gtk_label_set_markup (GTK_LABEL (label), markup);
  gtk_box_append (GTK_BOX (self->body), label);
  g_string_truncate (prose, 0);
}

/*
 * Markdown tables already become aligned box-drawing grids. Pull complete
 * table paragraphs out of prose so each grid gets the same padded, rounded
 * card as a fenced code block.
 */
static void
append_markdown_chunk (XdMessageRow *self,
                       const char   *text)
{
  g_auto (GStrv) lines = g_strsplit (text, "\n", -1);
  g_autoptr (GString) prose = g_string_new (NULL);
  guint line = 0;

  while (lines[line] != NULL)
    {
      g_autoptr (GString) paragraph = g_string_new (NULL);
      g_autofree char *grid = NULL;
      guint end;

      if (line_is_blank (lines[line]))
        {
          append_line (prose, lines[line]);
          line++;
          continue;
        }

      for (end = line; lines[end] != NULL && !line_is_blank (lines[end]); end++)
        append_line (paragraph, lines[end]);

      if (strchr (paragraph->str, '|') != NULL &&
          strchr (paragraph->str, '\n') != NULL)
        grid = xd_markdown_table_to_text (paragraph->str);
      if (grid == NULL)
        {
          for (; line < end; line++)
            append_line (prose, lines[line]);
          continue;
        }

      append_markdown_prose (self, prose);
      gtk_box_append (GTK_BOX (self->body),
                      make_code_card (self, grid, FALSE, FALSE));
      line = end;

      /* Body spacing replaces blank lines around the card. */
      while (lines[line] != NULL && line_is_blank (lines[line]))
        line++;
    }

  append_markdown_prose (self, prose);
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
                if (in_fence)
                  gtk_box_append (
                    GTK_BOX (self->body),
                    make_code_card (self, chunk->str, diff_fence, TRUE));
                else
                  append_markdown_chunk (self, chunk->str);
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
        if (in_fence)
          gtk_box_append (
            GTK_BOX (self->body),
            make_code_card (self, chunk->str, diff_fence, TRUE));
        else
          append_markdown_chunk (self, chunk->str);
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
