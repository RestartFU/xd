module Xd
  module UI
    enum SidebarState
      Idle
      Working
      Waiting
      Done
      Offline

      def reconcile_tree(
        working : Bool,
        active : Bool,
        remote : Bool,
      ) : self
        return Working if working
        return self unless working?

        remote || active ? Idle : Done
      end

      def finish(waiting : Bool, active : Bool) : self
        return Waiting if waiting

        active ? Idle : Done
      end

      def opened : self
        done? ? Idle : self
      end

      def answered : self
        waiting? ? Idle : self
      end
    end
  end
end
