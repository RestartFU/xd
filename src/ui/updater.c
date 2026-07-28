#include "updater.h"

#include "util/host-launch.h"
#include "util/update-channel.h"

/*
 * Asking GitHub, now and then, whether there is a newer build.
 *
 * Through curl rather than an HTTP client of our own: the update itself is the
 * install script, which needs curl anyway, so a machine that cannot run this
 * check could not have acted on it. It also keeps the certificate handling on
 * the host, where the certificates are.
 *
 * The bundle carries its own libraries and its launcher rewrote the
 * environment to match, so everything spawned here is given the host's
 * environment back -- curl is the host's, and must load the host's OpenSSL.
 */

/* Often enough that a build pushed while the window is open turns up in it --
 * how often is the channel's to say, since a dev build is watched far more
 * closely than a nightly. The first check waits a moment either way, because
 * starting up is busy and this is not. */
#define FIRST_CHECK_SECONDS 8

typedef enum
{
  STATE_QUIET,        /* nothing to say: up to date, or not installed */
  STATE_AVAILABLE,
  STATE_UPDATING,
  STATE_DONE,         /* installed; the running copy is still the old one */
  STATE_FAILED,
} State;

struct _XdUpdater
{
  AdwBin parent_instance;

  GtkButton *button;
  State state;
  char *trouble;

  /* Where the running copy lives, when it lives somewhere the installer would
   * replace. NULL otherwise, which is what keeps this quiet in a checkout. */
  char *install_dir;

  guint check_id;
  gboolean repeating;     /* the timer running now is the repeating one */
  GCancellable *cancellable;
};

G_DEFINE_FINAL_TYPE (XdUpdater, xd_updater, ADW_TYPE_BIN)

static gboolean check_now (gpointer user_data);

/* --- what this copy is ------------------------------------------------------ */

/*
 * The bundle the running process is part of, if it is one.
 *
 * The installer puts a bundle at ~/.local/opt/<name> and runs it through the
 * launcher beside the binary. Anything else -- a build tree, a tarball
 * unpacked somewhere to try -- is not the copy an update would replace, so
 * there is nothing useful to offer.
 */
static char *
find_install_dir (void)
{
  g_autofree char *self = g_file_read_link ("/proc/self/exe", NULL);
  g_autofree char *bin = NULL;
  g_autofree char *dir = NULL;
  g_autofree char *launcher = NULL;
  g_autofree char *expected = NULL;

  if (self == NULL)
    return NULL;

  bin = g_path_get_dirname (self);        /* …/<bundle>/bin */
  dir = g_path_get_dirname (bin);         /* …/<bundle>     */

  launcher = g_build_filename (dir, "xd.sh", NULL);
  if (!g_file_test (launcher, G_FILE_TEST_IS_EXECUTABLE))
    return NULL;

  /* And it is the one the installer manages, not a copy sitting elsewhere. */
  expected = g_build_filename (g_get_home_dir (), ".local", "opt",
                               XD_DATA_NAME, NULL);
  if (g_strcmp0 (dir, expected) != 0)
    return NULL;

  return g_steal_pointer (&dir);
}

/* --- saying so -------------------------------------------------------------- */

static void
show_state (XdUpdater *self)
{
  const char *icon = NULL;
  const char *accessible_label = NULL;
  const char *tip = NULL;
  gboolean fades = FALSE;

  switch (self->state)
    {
    case STATE_AVAILABLE:
      icon = "document-save-symbolic";
      accessible_label = "Download update";
      tip = "A newer build is available. Click to install it.";
      fades = TRUE;
      break;

    case STATE_UPDATING:
      icon = "document-save-symbolic";
      accessible_label = "Downloading update";
      tip = "Downloading and installing.";
      break;

    case STATE_DONE:
      icon = "view-refresh-symbolic";
      accessible_label = "Restart";
      tip = "The new build is installed. Click to restart into it.";
      break;

    case STATE_FAILED:
      icon = "document-save-symbolic";
      accessible_label = "Retry update";
      tip = self->trouble;
      fades = TRUE;
      break;

    case STATE_QUIET:
    default:
      break;
    }

  gtk_widget_set_visible (GTK_WIDGET (self), icon != NULL);

  if (icon == NULL)
    return;

  gtk_button_set_icon_name (self->button, icon);
  gtk_accessible_update_property (
    GTK_ACCESSIBLE (self->button),
    GTK_ACCESSIBLE_PROPERTY_LABEL, accessible_label,
    -1);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->button), tip);
  gtk_widget_set_sensitive (GTK_WIDGET (self->button),
                            self->state != STATE_UPDATING);
  gtk_widget_set_can_target (GTK_WIDGET (self->button),
                             self->state != STATE_UPDATING);

  if (fades)
    gtk_widget_add_css_class (GTK_WIDGET (self->button),
                              "xd-update-fade");
  else
    gtk_widget_remove_css_class (GTK_WIDGET (self->button),
                                 "xd-update-fade");
}

static void
set_state (XdUpdater *self,
           State      state)
{
  self->state = state;
  show_state (self);
}

/* --- running something on the host ------------------------------------------ */

static GSubprocess *
spawn (XdUpdater           *self,
       GSubprocessFlags     flags,
       const char *const   *argv,
       GError             **error)
{
  g_autoptr (GSubprocessLauncher) launcher = g_subprocess_launcher_new (flags);
  g_auto (GStrv) environment = xd_host_environ ();

  if (environment != NULL)
    g_subprocess_launcher_set_environ (launcher, environment);

  return g_subprocess_launcher_spawnv (launcher, argv, error);
}

/* --- is there a newer one? -------------------------------------------------- */

static void
on_checked (GObject      *source,
            GAsyncResult *result,
            gpointer      user_data)
{
  XdUpdater *self = user_data;
  g_autoptr (GBytes) out = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *latest = NULL;
  const char *body;
  gsize length;

  if (!g_subprocess_communicate_finish (G_SUBPROCESS (source), result,
                                        &out, NULL, &error))
    {
      /* No network, no GitHub, no curl: all the same thing here, and none of
       * them is worth saying to someone who did not ask. */
      g_debug ("update check failed: %s", error->message);
      return;
    }

  body = g_bytes_get_data (out, &length);
  if (body == NULL || length == 0)
    return;

  latest = xd_update_channel_latest_from_reply (
    xd_update_channel_current (), body);

  if (xd_update_channel_is_newer (xd_update_channel_current (), latest) &&
      self->state == STATE_QUIET)
    set_state (self, STATE_AVAILABLE);
}

static void
look (XdUpdater *self)
{
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *url = NULL;

  /* Nothing to look for while the answer is already "yes" or "restart". */
  if (self->state != STATE_QUIET || self->install_dir == NULL)
    return;

  url = xd_update_channel_check_url (xd_update_channel_current ());

  {
    const char *argv[] = {
      "curl", "-fsSL", "--max-time", "20",
      "-H", "Accept: application/vnd.github+json",
      url, NULL,
    };

    process = spawn (self, G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                           G_SUBPROCESS_FLAGS_STDERR_SILENCE,
                     argv, &error);
  }

  if (process == NULL)
    {
      g_debug ("cannot check for updates: %s", error->message);
      return;
    }

  g_subprocess_communicate_async (process, NULL, self->cancellable,
                                  on_checked, self);
}

/*
 * The first check is a one-shot, and every one after it repeats.
 *
 * Whichever timer is running is the one in check_id, so there is always
 * exactly one and disposing takes it with it.
 */
static gboolean
check_now (gpointer user_data)
{
  XdUpdater *self = user_data;

  look (self);

  if (self->repeating)
    return G_SOURCE_CONTINUE;

  self->repeating = TRUE;
  self->check_id = g_timeout_add_seconds (
    xd_update_channel_poll_seconds (xd_update_channel_current ()),
    check_now, self);

  return G_SOURCE_REMOVE;
}

/* --- installing it ---------------------------------------------------------- */

static void
on_installed (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  XdUpdater *self = user_data;
  g_autoptr (GError) error = NULL;

  if (g_subprocess_wait_check_finish (G_SUBPROCESS (source), result, &error))
    {
      set_state (self, STATE_DONE);
      return;
    }

  g_free (self->trouble);
  self->trouble = g_strdup (error->message);
  set_state (self, STATE_FAILED);
}

static void
install (XdUpdater *self)
{
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *line = NULL;

  /* Disable the action before doing any setup. The state is the lock: even
   * an input event already queued behind the click cannot start a second
   * installer. A launch failure changes it to the retry state below. */
  set_state (self, STATE_UPDATING);

  line = xd_update_channel_install_command (xd_update_channel_current ());

  {
    const char *argv[] = { "sh", "-c", line, NULL };

    process = spawn (self, G_SUBPROCESS_FLAGS_STDOUT_SILENCE, argv, &error);
  }

  if (process == NULL)
    {
      g_free (self->trouble);
      self->trouble = g_strdup (error->message);
      set_state (self, STATE_FAILED);
      return;
    }

  g_subprocess_wait_check_async (process, self->cancellable, on_installed, self);
}

/* Into the copy that was just installed, which is at the same path. */
static void
restart (XdUpdater *self)
{
  g_autofree char *launcher = g_build_filename (self->install_dir, "xd.sh", NULL);
  g_autoptr (GError) error = NULL;
  const char *argv[] = { launcher, NULL };
  g_autoptr (GSubprocess) process = spawn (self, G_SUBPROCESS_FLAGS_NONE,
                                           argv, &error);

  if (process == NULL)
    {
      g_free (self->trouble);
      self->trouble = g_strdup (error->message);
      set_state (self, STATE_FAILED);
      return;
    }

  /* The new one is starting; this one has nothing left to do. */
  g_application_quit (g_application_get_default ());
}

static void
on_clicked (GtkButton *button,
            gpointer   user_data)
{
  XdUpdater *self = user_data;

  switch (self->state)
    {
    case STATE_AVAILABLE:
    case STATE_FAILED:
      install (self);
      break;

    case STATE_DONE:
      restart (self);
      break;

    default:
      break;
    }
}

/* --- lifecycle -------------------------------------------------------------- */

XdUpdater *
xd_updater_new (void)
{
  return g_object_new (XD_TYPE_UPDATER, NULL);
}

static void
xd_updater_dispose (GObject *object)
{
  XdUpdater *self = XD_UPDATER (object);

  g_cancellable_cancel (self->cancellable);
  g_clear_object (&self->cancellable);
  g_clear_handle_id (&self->check_id, g_source_remove);

  G_OBJECT_CLASS (xd_updater_parent_class)->dispose (object);
}

static void
xd_updater_finalize (GObject *object)
{
  XdUpdater *self = XD_UPDATER (object);

  g_clear_pointer (&self->install_dir, g_free);
  g_clear_pointer (&self->trouble, g_free);

  G_OBJECT_CLASS (xd_updater_parent_class)->finalize (object);
}

static void
xd_updater_class_init (XdUpdaterClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->dispose = xd_updater_dispose;
  object_class->finalize = xd_updater_finalize;
}

static void
xd_updater_init (XdUpdater *self)
{
  self->cancellable = g_cancellable_new ();
  self->install_dir = find_install_dir ();
  self->state = STATE_QUIET;

  self->button = GTK_BUTTON (gtk_button_new ());
  gtk_widget_add_css_class (GTK_WIDGET (self->button), "suggested-action");
  gtk_widget_add_css_class (GTK_WIDGET (self->button), "xd-update");
  g_signal_connect (self->button, "clicked", G_CALLBACK (on_clicked), self);

  adw_bin_set_child (ADW_BIN (self), GTK_WIDGET (self->button));

  /* Invisible until there is something to say. */
  gtk_widget_set_visible (GTK_WIDGET (self), FALSE);

  if (self->install_dir != NULL)
    self->check_id = g_timeout_add_seconds (FIRST_CHECK_SECONDS, check_now, self);
}
