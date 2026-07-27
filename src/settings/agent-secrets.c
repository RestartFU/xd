#include "agent-secrets.h"

#include "util/app-paths.h"

#include <errno.h>
#include <glib/gstdio.h>
#include <json-glib/json-glib.h>
#include <stdlib.h>

struct _XdAgentSecrets
{
  char *path;
  GHashTable *values; /* name -> value */
};

static GQuark
secrets_error_quark (void)
{
  return g_quark_from_static_string ("xd-agent-secrets-error");
}

gboolean
xd_agent_secret_name_is_valid (const char *name)
{
  if (name == NULL || *name == '\0' ||
      !(g_ascii_isalpha (*name) || *name == '_'))
    return FALSE;

  for (const char *at = name + 1; *at != '\0'; at++)
    if (!(g_ascii_isalnum (*at) || *at == '_'))
      return FALSE;

  return TRUE;
}

static char *
default_path (void)
{
  const char *override = g_getenv ("XD_AGENT_SECRETS_FILE");

  if (override != NULL && *override != '\0')
    return g_strdup (override);

  return xd_app_agent_secrets_path ();
}

static XdAgentSecrets *
secrets_new (const char *path)
{
  XdAgentSecrets *self = g_new0 (XdAgentSecrets, 1);

  self->path = path != NULL ? g_strdup (path) : default_path ();
  self->values = g_hash_table_new_full (g_str_hash, g_str_equal, g_free, g_free);

  return self;
}

void
xd_agent_secrets_free (XdAgentSecrets *self)
{
  if (self == NULL)
    return;

  g_free (self->path);
  g_hash_table_unref (self->values);
  g_free (self);
}

XdAgentSecrets *
xd_agent_secrets_load (const char  *path,
                       GError     **error)
{
  g_autoptr (XdAgentSecrets) self = secrets_new (path);
  g_autoptr (JsonParser) parser = json_parser_new ();
  JsonObject *root;
  JsonObject *secrets;
  JsonObjectIter iter;
  const char *name;
  JsonNode *value;

  if (!g_file_test (self->path, G_FILE_TEST_EXISTS))
    return g_steal_pointer (&self);

  if (!json_parser_load_from_file (parser, self->path, error))
    return NULL;

  if (!JSON_NODE_HOLDS_OBJECT (json_parser_get_root (parser)))
    {
      g_set_error (error, secrets_error_quark (), 1,
                   "%s does not contain a JSON object", self->path);
      return NULL;
    }

  root = json_node_get_object (json_parser_get_root (parser));
  if (!json_object_has_member (root, "secrets") ||
      !JSON_NODE_HOLDS_OBJECT (json_object_get_member (root, "secrets")))
    {
      g_set_error (error, secrets_error_quark (), 1,
                   "%s has no secrets object", self->path);
      return NULL;
    }

  secrets = json_object_get_object_member (root, "secrets");
  json_object_iter_init (&iter, secrets);
  while (json_object_iter_next (&iter, &name, &value))
    {
      if (!xd_agent_secret_name_is_valid (name) ||
          !JSON_NODE_HOLDS_VALUE (value) ||
          json_node_get_value_type (value) != G_TYPE_STRING ||
          json_node_get_string (value) == NULL ||
          *json_node_get_string (value) == '\0')
        {
          g_set_error (error, secrets_error_quark (), 1,
                       "%s contains an invalid secret entry", self->path);
          return NULL;
        }

      g_hash_table_insert (self->values, g_strdup (name),
                           json_node_dup_string (value));
    }

#ifndef G_OS_WIN32
  /* Old versions or manual edits may have left broader permissions. */
  if (g_chmod (self->path, 0600) != 0)
    g_warning ("cannot restrict %s to the current user", self->path);
#endif

  return g_steal_pointer (&self);
}

static gint
compare_names (gconstpointer left,
               gconstpointer right)
{
  return g_strcmp0 (*(char *const *) left, *(char *const *) right);
}

GStrv
xd_agent_secrets_names (XdAgentSecrets *self)
{
  GHashTableIter iter;
  gpointer name;
  GStrv names;
  guint at = 0;

  g_return_val_if_fail (self != NULL, NULL);

  names = g_new0 (char *, g_hash_table_size (self->values) + 1);
  g_hash_table_iter_init (&iter, self->values);
  while (g_hash_table_iter_next (&iter, &name, NULL))
    names[at++] = g_strdup (name);

  qsort (names, at, sizeof (char *), compare_names);
  return names;
}

gboolean
xd_agent_secrets_contains (XdAgentSecrets *self,
                           const char     *name)
{
  g_return_val_if_fail (self != NULL, FALSE);

  return g_hash_table_contains (self->values, name);
}

gboolean
xd_agent_secrets_set (XdAgentSecrets  *self,
                      const char      *name,
                      const char      *value,
                      GError         **error)
{
  g_return_val_if_fail (self != NULL, FALSE);

  if (!xd_agent_secret_name_is_valid (name))
    {
      g_set_error_literal (error, secrets_error_quark (), 1,
                           "Secret names must use letters, numbers and "
                           "underscores, and cannot start with a number.");
      return FALSE;
    }

  if (value == NULL || *value == '\0')
    {
      g_set_error_literal (error, secrets_error_quark (), 1,
                           "A new secret needs a value.");
      return FALSE;
    }

  g_hash_table_replace (self->values, g_strdup (name), g_strdup (value));
  return TRUE;
}

void
xd_agent_secrets_remove (XdAgentSecrets *self,
                         const char     *name)
{
  g_return_if_fail (self != NULL);

  g_hash_table_remove (self->values, name);
}

gboolean
xd_agent_secrets_save (XdAgentSecrets  *self,
                       GError         **error)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonGenerator) generator = json_generator_new ();
  g_autoptr (JsonNode) root = NULL;
  g_auto (GStrv) names = NULL;
  g_autofree char *parent = NULL;
  g_autofree char *text = NULL;
  gsize length;

  g_return_val_if_fail (self != NULL, FALSE);

  parent = g_path_get_dirname (self->path);
  if (g_mkdir_with_parents (parent, 0700) != 0)
    {
      g_set_error (error, G_FILE_ERROR, g_file_error_from_errno (errno),
                   "Cannot create %s", parent);
      return FALSE;
    }

  names = xd_agent_secrets_names (self);
  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "version");
  json_builder_add_int_value (builder, 1);
  json_builder_set_member_name (builder, "secrets");
  json_builder_begin_object (builder);
  for (gsize i = 0; names[i] != NULL; i++)
    {
      json_builder_set_member_name (builder, names[i]);
      json_builder_add_string_value (
        builder, g_hash_table_lookup (self->values, names[i]));
    }
  json_builder_end_object (builder);
  json_builder_end_object (builder);

  root = json_builder_get_root (builder);
  json_generator_set_root (generator, root);
  json_generator_set_pretty (generator, TRUE);
  text = json_generator_to_data (generator, &length);

  return g_file_set_contents_full (
    self->path, text, length, G_FILE_SET_CONTENTS_CONSISTENT, 0600, error);
}

GStrv
xd_agent_secrets_apply_environment (XdAgentSecrets *self,
                                    GStrv           environment)
{
  GHashTableIter iter;
  gpointer name;
  gpointer value;

  g_return_val_if_fail (self != NULL, environment);

  g_hash_table_iter_init (&iter, self->values);
  while (g_hash_table_iter_next (&iter, &name, &value))
    environment = g_environ_setenv (environment, name, value, TRUE);

  return environment;
}

char *
xd_agent_secrets_prompt (XdAgentSecrets *self)
{
  g_auto (GStrv) names = NULL;
  g_autofree char *joined = NULL;

  g_return_val_if_fail (self != NULL, NULL);

  names = xd_agent_secrets_names (self);
  if (names[0] == NULL)
    return NULL;

  joined = g_strjoinv (", ", names);
  return g_strdup_printf (
    "[Agent secrets available as environment variables: %s. Their values are "
    "not included in this prompt. Use them when needed, and never print or "
    "expose their values.]", joined);
}
