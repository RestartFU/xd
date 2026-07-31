require "gtk4"

module Xd
  module UI
    # Per-device pane visibility, keyed by local/remote chat identity.
    #
    # GVariantDict always emits a{sv}; GSettings requires the schema's exact
    # a{su} type. Build dictionary entries directly, matching pane-state.c.
    module PaneState
      extend self

      None     = 0_u32
      Terminal = 1_u32
      Files    = 2_u32
      Diff     = 4_u32

      def fetch(
        states : GLib::Variant,
        key : String,
        fallback : UInt32 = None,
      ) : UInt32
        pointer = LibGLib.g_variant_lookup_value(
          states,
          key,
          Pointer(Void).null
        )
        return fallback if pointer.null?

        value = GLib::Variant.new(pointer, GICrystal::Transfer::Full)
        value.as_u32? || fallback
      end

      def update(
        states : GLib::Variant,
        key : String,
        state : UInt32,
      ) : GLib::Variant
        unless states.type_string == "a{su}"
          raise ArgumentError.new("Pane state must be an a{su} variant.")
        end
        raise ArgumentError.new("Pane state key cannot be empty.") if key.empty?

        builder = GLib::VariantBuilder.new(
          GLib::VariantType.new("a{su}")
        )
        LibGLib.g_variant_n_children(states).times do |index|
          entry = child(states, index)
          stored_key = child(entry, 0_u64).as_s
          next if stored_key == key

          stored_state = child(entry, 1_u64).as_u32
          builder.add_value(dict_entry(stored_key, stored_state))
        end
        builder.add_value(dict_entry(key, state))
        builder.end
      end

      private def child(
        variant : GLib::Variant,
        index : UInt64,
      ) : GLib::Variant
        pointer = LibGLib.g_variant_get_child_value(variant, index)
        GLib::Variant.new(pointer, GICrystal::Transfer::Full)
      end

      private def dict_entry(
        key : String,
        state : UInt32,
      ) : GLib::Variant
        key_variant = GLib::Variant.new(key)
        state_variant = GLib::Variant.new(state)
        pointer = LibGLib.g_variant_new_dict_entry(
          key_variant,
          state_variant
        )
        GLib::Variant.new(pointer, GICrystal::Transfer::None)
      end
    end
  end
end
