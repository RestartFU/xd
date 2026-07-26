#include "git-actions.h"

#include "util/host-launch.h"

/*
 * One button for the next thing to do with the repository.
 *
 * Work goes commit, push, open a pull request, and which of those is next is
 * a question the repository can answer. So the button offers that one and
 * keeps the others in a menu, rather than asking the user to work out which
 * applies and to remember the commands.
 */

typedef enum
{
  ACTION_NONE,
  ACTION_COMMIT,
  ACTION_PUSH,
  ACTION_PULL_REQUEST,
  ACTION_VIEW_PULL_REQUEST,
} GitAction;

struct _HyGitActions
{
  AdwBin parent_instance;

  char *workdir;
  GCancellable *cancellable;
  GitAction suggested;

  GtkButton *primary;
};

G_DEFINE_FINAL_TYPE (HyGitActions, hy_git_actions, ADW_TYPE_BIN)

/*
 * Everything the decision needs, in one run.
 *
 * Five separate spawns to answer one question would be five chances to catch
 * the repository mid-change, on top of being slower.
 */
static const char *STATE_SCRIPT =
  "printf '%s\\n' \"$(git status --porcelain 2>/dev/null | head -n 1)\"; "
  "git rev-parse --abbrev-ref HEAD 2>/dev/null || echo; "
  "git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null || echo; "
  "git rev-list --count '@{u}..HEAD' 2>/dev/null || echo 0; "
  "for ref in \"$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD)\" "
  "origin/main origin/master main master; do "
  "  [ -n \"$ref\" ] || continue; "
  "  git rev-parse --verify --quiet \"$ref\" >/dev/null && "
  "    { echo \"${ref##*/}\"; break; }; "
  "done; "
  "gh pr view --json url --jq .url 2>/dev/null || true";

static const char *
action_label (GitAction action)
{
  switch (action)
    {
    case ACTION_COMMIT:       return "Commit";
    case ACTION_PUSH:         return "Push";
    case ACTION_PULL_REQUEST:      return "Create PR";
    case ACTION_VIEW_PULL_REQUEST: return "View PR";
    default:                       return "Up to date";
    }
}

/* --- running things -------------------------------------------------------- */

static void
on_action_finished (GObject      *source,
                    GAsyncResult *result,
                    gpointer      user_data)
{
  HyGitActions *self = user_data;
  g_autofree char *output = NULL;
  g_autofree char *errors = NULL;
  g_autoptr (GError) error = NULL;

  if (!g_subprocess_communicate_utf8_finish (G_SUBPROCESS (source), result,
                                             &output, &errors, &error))
    return;

  if (!g_subprocess_get_successful (G_SUBPROCESS (source)))
    {
      AdwAlertDialog *dialog;
      const char *detail = errors != NULL && *errors != '\0' ? errors : output;

      dialog = ADW_ALERT_DIALOG (adw_alert_dialog_new ("Git Refused",
                                                       detail != NULL ? detail : NULL));
      adw_alert_dialog_add_response (dialog, "close", "Close");
      adw_dialog_present (ADW_DIALOG (dialog), GTK_WIDGET (self));
    }

  hy_git_actions_refresh (self);
}

/*
 * Runs @script in the working directory.
 *
 * The host environment goes with it: `gh` opens a browser, and a browser
 * launched under the bundle's GTK and schemas is a different program than the
 * one the user configured.
 */
static void
run_script (HyGitActions        *self,
            const char          *script,
            const char          *argument,
            GAsyncReadyCallback  callback)
{
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;
  g_auto (GStrv) env = hy_host_environ ();

  if (self->workdir == NULL)
    return;

  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                                        G_SUBPROCESS_FLAGS_STDERR_PIPE);
  g_subprocess_launcher_set_cwd (launcher, self->workdir);
  g_subprocess_launcher_set_environ (launcher, env);

  /* "sh" fills $0, so the argument lands in $1 rather than being pasted into
   * the script -- a commit message is arbitrary text. */
  if (argument != NULL)
    process = g_subprocess_launcher_spawn (launcher, &error, "sh", "-c", script,
                                           "sh", argument, NULL);
  else
    process = g_subprocess_launcher_spawn (launcher, &error, "sh", "-c", script, NULL);

  if (process == NULL)
    {
      g_debug ("cannot run git: %s", error->message);
      return;
    }

  g_subprocess_communicate_utf8_async (process, NULL, self->cancellable,
                                       callback, self);
}

/* --- deciding what to offer ------------------------------------------------ */

static void
on_state_read (GObject      *source,
               GAsyncResult *result,
               gpointer      user_data)
{
  HyGitActions *self = user_data;
  g_autofree char *output = NULL;
  g_autoptr (GError) error = NULL;
  g_auto (GStrv) lines = NULL;
  const char *dirty, *branch, *upstream, *base, *pr_url;
  int ahead;

  if (!g_subprocess_communicate_utf8_finish (G_SUBPROCESS (source), result,
                                             &output, NULL, &error))
    return;

  lines = g_strsplit (output != NULL ? output : "", "\n", -1);
  if (g_strv_length (lines) < 5)
    {
      gtk_widget_set_visible (GTK_WIDGET (self), FALSE);
      return;
    }

  dirty    = lines[0];
  branch   = lines[1];
  upstream = lines[2];
  ahead    = atoi (lines[3]);
  base     = lines[4];
  pr_url   = g_strv_length (lines) > 5 ? lines[5] : "";

  /* Not a repository, or one with no commits to speak of. */
  if (*branch == '\0')
    {
      gtk_widget_set_visible (GTK_WIDGET (self), FALSE);
      return;
    }

  gtk_widget_set_visible (GTK_WIDGET (self), TRUE);

  if (*dirty != '\0')
    self->suggested = ACTION_COMMIT;
  else if (ahead > 0 || *upstream == '\0')
    self->suggested = ACTION_PUSH;
  else if (g_strcmp0 (branch, base) != 0)
    /* On a branch of its own, with everything pushed. If the ask has already
     * been made, the useful thing is the conversation happening on it. */
    self->suggested = *pr_url != '\0' ? ACTION_VIEW_PULL_REQUEST
                                       : ACTION_PULL_REQUEST;
  else
    self->suggested = ACTION_NONE;

  gtk_button_set_label (self->primary, action_label (self->suggested));
  gtk_widget_set_sensitive (GTK_WIDGET (self->primary),
                            self->suggested != ACTION_NONE);
}

void
hy_git_actions_refresh (HyGitActions *self)
{
  g_return_if_fail (HY_IS_GIT_ACTIONS (self));

  g_cancellable_cancel (self->cancellable);
  g_clear_object (&self->cancellable);
  self->cancellable = g_cancellable_new ();

  if (self->workdir == NULL)
    {
      gtk_widget_set_visible (GTK_WIDGET (self), FALSE);
      return;
    }

  run_script (self, STATE_SCRIPT, NULL, on_state_read);
}

void
hy_git_actions_set_workdir (HyGitActions *self,
                            const char   *workdir)
{
  g_return_if_fail (HY_IS_GIT_ACTIONS (self));

  if (g_strcmp0 (self->workdir, workdir) == 0)
    return;

  g_free (self->workdir);
  self->workdir = g_strdup (workdir);

  hy_git_actions_refresh (self);
}

/* --- the actions themselves ------------------------------------------------ */

static void
on_message_written (GObject      *source,
                    GAsyncResult *result,
                    gpointer      user_data)
{
  HyGitActions *self = user_data;
  AdwAlertDialog *dialog = ADW_ALERT_DIALOG (source);
  const char *response = adw_alert_dialog_choose_finish (dialog, result);
  GtkEditable *entry = g_object_get_data (G_OBJECT (dialog), "entry");
  const char *message = gtk_editable_get_text (entry);

  if (g_strcmp0 (response, "commit") != 0 || *message == '\0')
    return;

  /* Everything that changed, since the pane beside it shows exactly that and
   * choosing a subset is what a full git client is for. */
  run_script (self, "git add -A && git commit -m \"$1\"", message,
              on_action_finished);
}

static void
commit (HyGitActions *self)
{
  AdwAlertDialog *dialog =
    ADW_ALERT_DIALOG (adw_alert_dialog_new ("Commit Everything Changed", NULL));
  GtkWidget *group = adw_preferences_group_new ();
  GtkWidget *row = adw_entry_row_new ();

  adw_preferences_row_set_title (ADW_PREFERENCES_ROW (row), "Message");
  adw_preferences_group_add (ADW_PREFERENCES_GROUP (group), row);
  adw_alert_dialog_set_extra_child (dialog, group);

  adw_alert_dialog_add_responses (dialog, "cancel", "Cancel",
                                  "commit", "Commit", NULL);
  adw_alert_dialog_set_response_appearance (dialog, "commit",
                                            ADW_RESPONSE_SUGGESTED);
  adw_alert_dialog_set_default_response (dialog, "commit");
  adw_alert_dialog_set_close_response (dialog, "cancel");

  g_object_set_data (G_OBJECT (dialog), "entry", row);
  adw_alert_dialog_choose (dialog, GTK_WIDGET (self), NULL,
                           on_message_written, self);
}

static void
run_action (HyGitActions *self,
            GitAction     action)
{
  switch (action)
    {
    case ACTION_COMMIT:
      commit (self);
      break;

    case ACTION_PUSH:
      /* -u on every push, so a branch that has never been pushed works the
       * same as one that has. */
      run_script (self, "git push -u origin HEAD", NULL, on_action_finished);
      break;

    case ACTION_PULL_REQUEST:
      /* --web rather than creating it outright: the title and body are worth
       * seeing before it exists, and gh knows how to open a browser. */
      run_script (self, "gh pr create --web", NULL, on_action_finished);
      break;

    case ACTION_VIEW_PULL_REQUEST:
      run_script (self, "gh pr view --web", NULL, on_action_finished);
      break;

    default:
      break;
    }
}

static void
on_primary_clicked (GtkButton *button,
                    gpointer   user_data)
{
  HyGitActions *self = user_data;

  run_action (self, self->suggested);
}

HyGitActions *
hy_git_actions_new (void)
{
  return g_object_new (HY_TYPE_GIT_ACTIONS, NULL);
}

static void
hy_git_actions_dispose (GObject *object)
{
  HyGitActions *self = HY_GIT_ACTIONS (object);

  g_cancellable_cancel (self->cancellable);
  g_clear_object (&self->cancellable);
  g_clear_pointer (&self->workdir, g_free);

  G_OBJECT_CLASS (hy_git_actions_parent_class)->dispose (object);
}

static void
hy_git_actions_class_init (HyGitActionsClass *klass)
{
  G_OBJECT_CLASS (klass)->dispose = hy_git_actions_dispose;
}

static void
hy_git_actions_init (HyGitActions *self)
{
  /* Words, no icon. An icon the theme does not carry draws as a broken-image
   * box, and there is no icon for "create a pull request" that is clearer
   * than saying so. */
  self->primary = GTK_BUTTON (gtk_button_new_with_label (action_label (ACTION_NONE)));
  gtk_widget_add_css_class (GTK_WIDGET (self->primary), "flat");
  g_signal_connect (self->primary, "clicked", G_CALLBACK (on_primary_clicked), self);

  /*
   * One button, no menu behind it.
   *
   * The three actions are a sequence, not a choice: work is committed, then
   * pushed, then asked to be merged, and the repository already says which
   * one is next. A menu offering the other two offered doing them out of
   * order, which git would refuse anyway.
   */
  gtk_widget_set_visible (GTK_WIDGET (self), FALSE);

  adw_bin_set_child (ADW_BIN (self), GTK_WIDGET (self->primary));
}
