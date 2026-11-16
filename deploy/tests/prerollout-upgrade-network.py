#!/usr/bin/env python3
"""0.1.7 -> 0.1.8 upgrade: network redefinitions must not strand reused containers.

M8 acceptance found that switching the backend network to `internal: true` made Compose replace the network
during `up api` while restarting the earlier migrate container, still attached to the removed network; it could
not resolve postgres and the upgrade rolled back. The installer must take the old project down after the
pre-upgrade dump and before switching, and rollback must bring postgres up itself after doing the same.
"""
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
INSTALL = (ROOT / 'deploy/install.sh').read_text()


def body(start, end):
    i = INSTALL.index(start)
    return INSTALL[i:INSTALL.index(end, i)]


class UpgradeNetworkTests(unittest.TestCase):
    def test_upgrade_takes_old_project_down_after_dump_before_switch(self):
        upgrade = body('dc exec -T postgres pg_dump', 'upgrade_pending=0\n    rm -f -- "$rollback_dump"')
        down = upgrade.find('dc down --remove-orphans')
        self.assertGreater(down, 0, 'upgrade must remove old containers/networks before the new release starts')
        for later in ('set_version', 'switch_release', '\n      migrate\n', '\n      start\n'):
            self.assertGreater(upgrade.index(later), down, f'{later.strip()} must follow the project down')

    def test_rollback_restarts_postgres_after_project_down(self):
        rollback = body('rollback_upgrade() {', '\n}\n')
        down = rollback.find('dc down --remove-orphans')
        up = rollback.find('dc up -d --wait postgres')
        self.assertGreater(down, 0, 'rollback must drop containers created with the new definitions')
        self.assertRegex(rollback[down:rollback.index('\n', down)], r'\|\| true', 'rollback down is best effort')
        self.assertGreater(up, down, 'postgres must be started after the project down')
        self.assertLess(up, rollback.index('restore_db'), 'database must be up before restore')
        self.assertLess(up, rollback.index('start --no-deps'), 'database must be up before the API')

    def test_down_never_removes_volumes(self):
        for match in re.finditer(r'dc down[^\n]*', INSTALL):
            self.assertNotRegex(match.group(0), r'\s(-v|--volumes)\b', 'bind-mounted data must survive')


if __name__ == '__main__':
    unittest.main()
