#include "folder-settings.h"

#include <gio/gio.h>
#include <json-glib/json-glib.h>

HyFolderSettings *
hy_folder_settings_new (void)
{
  return g_new0 (HyFolderSettings, 1);
}

void
hy_folder_settings_free (HyFolderSettings *self)
{
  if (self == NULL)
    return;

  g_free (self->id);
  g_free (self->backend);
  g_free (self->model);
  g_free (self->workdir);
  g_free (self->repo);
  g_free (self->instructions);
  g_free (self);
}

static char *
settings_path_for (const char *folder_path)
{
  return g_build_filename (folder_path, HY_FOLDER_SETTINGS_FILE, NULL);
}

/* Returns a copy of a member, or NULL for both "absent" and JSON null. */
static char *
dup_member (JsonObject *object,
            const char *name)
{
  JsonNode *node;

  if (!json_object_has_member (object, name))
    return NULL;

  node = json_object_get_member (object, name);
  if (JSON_NODE_HOLDS_NULL (node) || !JSON_NODE_HOLDS_VALUE (node))
    return NULL;

  return json_node_dup_string (node);
}

HyFolderSettings *
hy_folder_settings_load (const char  *folder_path,
                         GError     **error)
{
  g_autoptr (JsonParser) parser = json_parser_new ();
  g_autofree char *path = settings_path_for (folder_path);
  HyFolderSettings *self;
  JsonObject *object;
  JsonNode *root;

  if (!json_parser_load_from_file (parser, path, error))
    return NULL;

  root = json_parser_get_root (parser);
  if (root == NULL || !JSON_NODE_HOLDS_OBJECT (root))
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                   "%s does not contain a JSON object", path);
      return NULL;
    }

  object = json_node_get_object (root);

  self = hy_folder_settings_new ();
  self->id           = dup_member (object, "id");
  self->backend      = dup_member (object, "backend");
  self->model        = dup_member (object, "model");
  self->workdir      = dup_member (object, "workdir");
  self->repo         = dup_member (object, "repo");
  self->instructions = dup_member (object, "instructions");

  if (self->id == NULL)
    self->id = g_uuid_string_random ();

  return self;
}

HyFolderSettings *
hy_folder_settings_ensure (const char  *folder_path,
                           GError     **error)
{
  g_autofree char *path = settings_path_for (folder_path);
  HyFolderSettings *self;

  if (g_file_test (path, G_FILE_TEST_EXISTS))
    {
      g_autoptr (GError) local_error = NULL;

      self = hy_folder_settings_load (folder_path, &local_error);
      if (self != NULL)
        return self;

      /* A corrupt file must not make the folder unusable: warn, then replace
       * it so the folder still gets a stable identity. */
      g_warning ("ignoring unreadable %s: %s", path, local_error->message);
    }

  self = hy_folder_settings_new ();
  self->id = g_uuid_string_random ();

  if (!hy_folder_settings_save (self, folder_path, error))
    {
      hy_folder_settings_free (self);
      return NULL;
    }

  return self;
}

static void
add_member (JsonBuilder *builder,
            const char  *name,
            const char  *value)
{
  json_builder_set_member_name (builder, name);

  if (value != NULL)
    json_builder_add_string_value (builder, value);
  else
    json_builder_add_null_value (builder);
}

gboolean
hy_folder_settings_save (const HyFolderSettings  *self,
                         const char              *folder_path,
                         GError                 **error)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonGenerator) generator = json_generator_new ();
  g_autoptr (JsonNode) root = NULL;
  g_autofree char *path = settings_path_for (folder_path);
  g_autofree char *text = NULL;
  gsize length;

  g_return_val_if_fail (self != NULL, FALSE);
  g_return_val_if_fail (self->id != NULL, FALSE);

  json_builder_begin_object (builder);
  add_member (builder, "id", self->id);
  add_member (builder, "backend", self->backend);
  add_member (builder, "model", self->model);
  add_member (builder, "workdir", self->workdir);
  add_member (builder, "repo", self->repo);
  add_member (builder, "instructions", self->instructions);
  json_builder_end_object (builder);

  root = json_builder_get_root (builder);
  json_generator_set_root (generator, root);
  json_generator_set_pretty (generator, TRUE);
  text = json_generator_to_data (generator, &length);

  return g_file_set_contents (path, text, length, error);
}
