// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";
import {Offer} from "../../src/types/Offer.sol";

contract OfferTest is Test {
    /// forge-config: default.allow_internal_expect_revert = true
    function testBlockAtPrice() public {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(100),
            rampUpPeriod: 100,
            timeout: uint32(500),
            lockStake: 0.1 ether
        });

        assertEq(offer.blockAtPrice(1 ether), 0);

        assertEq(offer.blockAtPrice(1.01 ether), 101);
        assertEq(offer.blockAtPrice(1.001 ether), 101);

        assertEq(offer.blockAtPrice(1.25 ether), 125);
        assertEq(offer.blockAtPrice(1.5 ether), 150);
        assertEq(offer.blockAtPrice(1.75 ether), 175);
        assertEq(offer.blockAtPrice(1.99 ether), 199);

        assertEq(offer.blockAtPrice(2 ether), 200);

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InvalidRequest.selector));
        offer.blockAtPrice(3 ether);
    }

    function testPriceAtBlock() public pure {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(100),
            rampUpPeriod: 100,
            timeout: uint32(500),
            lockStake: 0.1 ether
        });

        assertEq(offer.priceAtBlock(0), 1 ether);
        assertEq(offer.priceAtBlock(100), 1 ether);

        assertEq(offer.priceAtBlock(101), 1.01 ether);
        assertEq(offer.priceAtBlock(125), 1.25 ether);
        assertEq(offer.priceAtBlock(150), 1.5 ether);
        assertEq(offer.priceAtBlock(175), 1.75 ether);
        assertEq(offer.priceAtBlock(199), 1.99 ether);

        assertEq(offer.priceAtBlock(200), 2 ether);
        assertEq(offer.priceAtBlock(500), 2 ether);
    }
}
