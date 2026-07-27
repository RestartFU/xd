#include "pane-state.h"

GVariant *
xd_pane_state_update (GVariant   *states,
                      const char *key,
                      guint       state)
{
  GVariantBuilder builder;
  GVariantIter iterator;
  const char *stored_key;
  guint stored_state;

  g_return_val_if_fail (
    g_variant_is_of_type (states, G_VARIANT_TYPE ("a{su}")), NULL);
  g_return_val_if_fail (key != NULL, NULL);

  g_variant_builder_init (&builder, G_VARIANT_TYPE ("a{su}"));
  g_variant_iter_init (&iterator, states);

  while (g_variant_iter_next (
           &iterator, "{&su}", &stored_key, &stored_state))
    {
      if (g_strcmp0 (stored_key, key) != 0)
        g_variant_builder_add (
          &builder, "{su}", stored_key, stored_state);
    }

  g_variant_builder_add (&builder, "{su}", key, state);

  return g_variant_ref_sink (g_variant_builder_end (&builder));
}
