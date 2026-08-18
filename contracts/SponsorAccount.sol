// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

/// @title SponsorAccount — EIP-7702 delegation target for gas-fee sponsorship.
///
/// Users delegate their EOA to this contract (EIP-7702). From then on, calls
/// TO the user's EOA execute this code in the user's context. The designated
/// sponsor can push `execute` calls from the user's account while paying the
/// gas itself; the user can also call their own EOA directly.
///
/// The sponsor cannot take funds: `execute` only runs calls the sponsor
/// explicitly builds (quotes come from the trading bot) and any value sent is
/// explicitly specified per call.
contract SponsorAccount {
    address public immutable sponsor;

    constructor(address sponsor_) {
        sponsor = sponsor_;
    }

    modifier onlySponsorOrSelf() {
        require(msg.sender == sponsor || msg.sender == address(this), "not authorized");
        _;
    }

    receive() external payable {}

    /// Execute `data` on `target` in the context of the delegated EOA.
    /// The CALLER pays the gas (normally the sponsor).
    function execute(address target, uint256 value, bytes calldata data)
        external
        onlySponsorOrSelf
        returns (bytes memory)
    {
        (bool ok, bytes memory ret) = target.call{value: value}(data);
        if (!ok) {
            // Bubble up the revert reason.
            assembly {
                revert(add(ret, 32), mload(ret))
            }
        }
        return ret;
    }

    function version() external pure returns (string memory) {
        return "1";
    }
}
