// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {SponsorAccount} from "../SponsorAccount.sol";

contract MockTarget {
    uint256 public lastValue;

    function record(uint256 v) external payable {
        lastValue = v;
    }

    function explode() external pure {
        revert("boom");
    }
}

/// Covers the security surface of the EIP-7702 delegation target:
/// only the sponsor (or the account itself, i.e. the delegated user) may
/// drive calls, reverts bubble up, and value can be pushed through.
contract SponsorAccountTest is Test {
    SponsorAccount account;
    MockTarget target;
    address sponsor = makeAddr("sponsor");
    address stranger = makeAddr("stranger");

    function setUp() public {
        account = new SponsorAccount(sponsor);
        target = new MockTarget();
    }

    function test_immutable_sponsor_is_set() public view {
        assertEq(account.sponsor(), sponsor);
    }

    function test_sponsor_can_execute_with_value() public {
        // The account forwards its own balance: value is a parameter of the
        // inner call, not msg.value.
        vm.deal(address(account), 1 ether);
        vm.prank(sponsor);
        account.execute(
            address(target),
            1 ether,
            abi.encodeWithSelector(MockTarget.record.selector, 42)
        );
        assertEq(target.lastValue(), 42);
        assertEq(address(target).balance, 1 ether);
    }

    function test_delegated_user_self_can_execute() public {
        // When the user calls their own delegated EOA, msg.sender is the
        // account itself (EIP-7702 execution context).
        vm.prank(address(account));
        account.execute(
            address(target),
            0,
            abi.encodeWithSelector(MockTarget.record.selector, 7)
        );
        assertEq(target.lastValue(), 7);
    }

    function test_stranger_is_rejected() public {
        vm.prank(stranger);
        vm.expectRevert("not authorized");
        account.execute(
            address(target),
            0,
            abi.encodeWithSelector(MockTarget.record.selector, 1)
        );
    }

    function test_revert_reason_bubbles_up() public {
        vm.prank(sponsor);
        vm.expectRevert(bytes("boom"));
        account.execute(address(target), 0, abi.encodeWithSelector(MockTarget.explode.selector));
    }

    function test_accepts_plain_eth_transfers() public {
        vm.deal(address(this), 1 ether);
        (bool ok,) = address(account).call{value: 1 ether}("");
        assertTrue(ok);
        assertEq(address(account).balance, 1 ether);
    }
}
