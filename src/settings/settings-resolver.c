#include "settings-resolver.h"

#include "folder-settings.h"

void
xd_effective_settings_free (XdEffectiveSettings *self)
{
  if (self == NULL)
    return;

  g_free (self->backend);
  g_free (self->model);
  g_free (self->workdir);
  g_free (self->repo);
  g_free (self->instructions);
  g_free (self->backend_from);
  g_free (self->model_from);
  g_free (self->workdir_from);
  g_free (self->repo_from);
  g_free (self);
}

/* Takes the first value seen, which -- walking from the folder upwards -- is
 * the nearest one. */
static void
take_nearest (char       **slot,
              char       **source_slot,
              const char  *value,
              const char  *folder_name,
              gboolean     is_self)
{
  if (*slot != NULL || value == NULL || *value == '\0')
    return;

  *slot = g_strdup (value);

  if (!is_self)
    *source_slot = g_strdup (folder_name);
}

XdEffectiveSettings *
xd_settings_resolve (XdNode     *folder,
                     const char *default_backend)
{
  XdEffectiveSettings *self;
  g_autoptr (GPtrArray) instructions = NULL;
  XdNode *node;
  gboolean is_self = TRUE;

  g_return_val_if_fail (folder == NULL || XD_IS_NODE (folder), NULL);

  self = g_new0 (XdEffectiveSettings, 1);
  instructions = g_ptr_array_new_with_free_func (g_free);

  for (node = folder; node != NULL; node = xd_node_get_parent (node))
    {
      g_autoptr (XdFolderSettings) settings = NULL;
      g_autoptr (GError) error = NULL;
      const char *name;

      if (xd_node_get_kind (node) != XD_NODE_FOLDER ||
          xd_node_get_folder_id (node) == NULL)
        continue;

      settings = xd_folder_settings_load (xd_node_get_path (node), &error);
      if (settings == NULL)
        {
          g_debug ("no settings for %s: %s", xd_node_get_path (node),
                   error->message);
          is_self = FALSE;
          continue;
        }

      name = xd_node_get_name (node);

      take_nearest (&self->backend, &self->backend_from, settings->backend,
                    name, is_self);
      take_nearest (&self->model, &self->model_from, settings->model,
                    name, is_self);
      take_nearest (&self->workdir, &self->workdir_from, settings->workdir,
                    name, is_self);
      take_nearest (&self->repo, &self->repo_from, settings->repo, name, is_self);

      if (settings->instructions != NULL && *settings->instructions != '\0')
        g_ptr_array_add (instructions, g_strdup (settings->instructions));

      is_self = FALSE;
    }

  /* Collected leaf-first; the model should read them the way they were
   * written, outermost context first. */
  if (instructions->len > 0)
    {
      GString *joined = g_string_new (NULL);

      for (guint i = instructions->len; i > 0; i--)
        {
          if (joined->len > 0)
            g_string_append (joined, "\n\n");

          g_string_append (joined, g_ptr_array_index (instructions, i - 1));
        }

      self->instructions = g_string_free (joined, FALSE);
    }

  /* A folder with no working directory of its own runs where it lives, which
   * keeps a chat pointed at something real even before anything is set. */
  if (self->workdir == NULL)
    {
      if (self->repo != NULL)
        self->workdir = g_strdup (self->repo);
      else if (folder != NULL && xd_node_get_path (folder) != NULL)
        self->workdir = g_strdup (xd_node_get_path (folder));
    }

  if (self->backend == NULL)
    self->backend = g_strdup (default_backend != NULL ? default_backend : "claude");

  return self;
}

char *
xd_settings_describe_place (XdNode     *folder,
                            const char *workdir)
{
  g_autoptr (GPtrArray) names = g_ptr_array_new ();
  g_autofree char *chain = NULL;

  if (folder == NULL || !XD_IS_NODE (folder))
    return NULL;

  /* Leaf first walking up, so the array is filled backwards and read out the
   * way the sidebar shows it. */
  for (XdNode *at = folder; at != NULL; at = xd_node_get_parent (at))
    {
      if (xd_node_get_kind (at) == XD_NODE_FOLDER &&
          xd_node_get_folder_id (at) != NULL)
        g_ptr_array_insert (names, 0, (gpointer) xd_node_get_name (at));
    }

  if (names->len == 0)
    return NULL;

  g_ptr_array_add (names, NULL);
  chain = g_strjoinv (" / ", (char **) names->pdata);

  if (workdir == NULL || *workdir == '\0')
    return g_strdup_printf ("[This conversation belongs to the folder \u201c%s\u201d "
                            "in the user\u2019s xd workspace tree.]", chain);

  return g_strdup_printf ("[This conversation belongs to the folder \u201c%s\u201d "
                          "in the user\u2019s xd workspace tree, and you are "
                          "running in %s. If that directory holds nothing but a "
                          "dotfile, it is the folder itself rather than a "
                          "checkout: say so and ask which repository is meant, "
                          "instead of searching the machine for one.]",
                          chain, workdir);
}
